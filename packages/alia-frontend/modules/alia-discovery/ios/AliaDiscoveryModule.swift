import CoreBluetooth
import ExpoModulesCore
import Foundation

private let aliaServerServiceType = "_alia-server._tcp."
private let aliaServerDomain = "local."
private let embeddedServiceUUID = CBUUID(string: "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f1")
private let networkStatusUUID = CBUUID(string: "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f2")
private let networkConfigUUID = CBUUID(string: "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f3")

public class AliaDiscoveryModule: Module {
  private var serverScans: [ServerScan] = []
  private let bleManager = AliaBleManager()

  public func definition() -> ModuleDefinition {
    Name("AliaDiscovery")

    AsyncFunction("scanServers") { (options: [String: Any]?, promise: Promise) in
      let timeoutMs = options?["timeoutMs"] as? Int ?? 3500
      var scan: ServerScan?
      scan = ServerScan(timeoutMs: timeoutMs, promise: promise) { [weak self] finished in
        self?.serverScans.removeAll { $0 === finished }
        scan = nil
      }
      if let scan {
        self.serverScans.append(scan)
        scan.start()
      }
    }.runOnQueue(.main)

    AsyncFunction("scanEmbedded") { (options: [String: Any]?, promise: Promise) in
      let timeoutMs = options?["timeoutMs"] as? Int ?? 5000
      self.bleManager.scanEmbedded(timeoutMs: timeoutMs, promise: promise)
    }.runOnQueue(.main)

    AsyncFunction("provisionEmbeddedNetwork") { (request: [String: Any], promise: Promise) in
      self.bleManager.provision(request: request, promise: promise)
    }.runOnQueue(.main)

    AsyncFunction("readEmbeddedNetworkStatus") { (request: [String: Any], promise: Promise) in
      self.bleManager.readStatus(request: request, promise: promise)
    }.runOnQueue(.main)
  }
}

private final class ServerScan: NSObject, NetServiceBrowserDelegate, NetServiceDelegate {
  private let browser = NetServiceBrowser()
  private let timeoutMs: Int
  private let promise: Promise
  private let onFinish: (ServerScan) -> Void
  private var services: [NetService] = []
  private var results: [String: [String: Any]] = [:]
  private var completed = false

  init(timeoutMs: Int, promise: Promise, onFinish: @escaping (ServerScan) -> Void) {
    self.timeoutMs = max(500, timeoutMs)
    self.promise = promise
    self.onFinish = onFinish
  }

  func start() {
    browser.delegate = self
    browser.searchForServices(ofType: aliaServerServiceType, inDomain: aliaServerDomain)
    DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(timeoutMs)) { [weak self] in
      self?.finish()
    }
  }

  func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
    services.append(service)
    service.delegate = self
    service.resolve(withTimeout: 2)
  }

  func netServiceDidResolveAddress(_ sender: NetService) {
    record(sender)
  }

  func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
    record(sender)
  }

  func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String: NSNumber]) {
    if completed {
      return
    }
    completed = true
    promise.reject("ERR_MDNS_SCAN", "Unable to scan for Alia Server on the local network.")
    onFinish(self)
  }

  private func finish() {
    if completed {
      return
    }
    completed = true
    browser.stop()
    promise.resolve(Array(results.values))
    onFinish(self)
  }

  private func record(_ service: NetService) {
    let txt = parseTxt(service.txtRecordData())
    let host = service.hostName
    let port = service.port > 0 ? service.port : nil
    let endpoint = txt["endpoint"] ?? endpointFrom(host: host, port: port)
    guard let endpoint else {
      return
    }

    let id = "\(service.name)|\(endpoint)"
    results[id] = [
      "id": id,
      "name": service.name,
      "endpoint": endpoint,
      "host": host as Any,
      "port": port as Any,
      "mode": txt["mode"] as Any,
      "txt": txt
    ].compactMapValues { value in
      if value is NSNull {
        return nil
      }
      return value
    }
  }

  private func endpointFrom(host: String?, port: Int?) -> String? {
    guard let host, let port else {
      return nil
    }
    return "http://\(host):\(port)"
  }

  private func parseTxt(_ data: Data?) -> [String: String] {
    guard let data else {
      return [:]
    }
    let dict = NetService.dictionary(fromTXTRecord: data)
    return dict.reduce(into: [String: String]()) { result, pair in
      result[pair.key] = String(data: pair.value, encoding: .utf8) ?? ""
    }
  }
}

private final class AliaBleManager: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
  private lazy var central = CBCentralManager(delegate: self, queue: .main)
  private var peripherals: [String: CBPeripheral] = [:]
  private var scanPromise: Promise?
  private var pendingScanTimeoutMs: Int?
  private var scanResults: [String: [String: Any]] = [:]
  private var pendingOperation: PendingOperation?

  func scanEmbedded(timeoutMs: Int, promise: Promise) {
    guard scanPromise == nil && pendingScanTimeoutMs == nil else {
      promise.reject("ERR_BLE_BUSY", "A BLE scan is already running.")
      return
    }

    scanPromise = promise
    scanResults = [:]

    if central.state == .poweredOn {
      startBleScan(timeoutMs: timeoutMs)
      return
    }

    if central.state == .unknown || central.state == .resetting {
      pendingScanTimeoutMs = timeoutMs
      DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(3)) { [weak self] in
        guard let self, self.pendingScanTimeoutMs != nil else {
          return
        }
        self.pendingScanTimeoutMs = nil
        self.rejectScan("ERR_BLE_UNAVAILABLE", self.bluetoothStateMessage())
      }
      return
    }

    scanPromise = nil
    scanResults = [:]
    promise.reject("ERR_BLE_UNAVAILABLE", bluetoothStateMessage())
  }

  private func startBleScan(timeoutMs: Int) {
    guard scanPromise != nil else {
      return
    }
    guard central.state == .poweredOn else {
      rejectScan("ERR_BLE_UNAVAILABLE", bluetoothStateMessage())
      return
    }

    central.scanForPeripherals(withServices: [embeddedServiceUUID], options: [
      CBCentralManagerScanOptionAllowDuplicatesKey: false
    ])

    DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(max(500, timeoutMs))) { [weak self] in
      self?.finishScan()
    }
  }

  private func rejectScan(_ code: String, _ message: String) {
    guard let promise = scanPromise else {
      return
    }
    central.stopScan()
    scanPromise = nil
    pendingScanTimeoutMs = nil
    scanResults = [:]
    promise.reject(code, message)
  }

  private func finishScan() {
    guard let promise = scanPromise else {
      return
    }
    central.stopScan()
    promise.resolve(Array(scanResults.values))
    scanPromise = nil
    pendingScanTimeoutMs = nil
  }

  private func guardBluetoothReady(_ promise: Promise) -> Bool {
    guard central.state == .poweredOn else {
      promise.reject("ERR_BLE_UNAVAILABLE", bluetoothStateMessage())
      return false
    }
    return true
  }

  func provision(request: [String: Any], promise: Promise) {
    guard guardBluetoothReady(promise) else {
      return
    }
    guard let nativeId = request["nativeId"] as? String, !nativeId.isEmpty else {
      promise.reject("ERR_BLE_REQUEST", "nativeId is required.")
      return
    }
    guard let ssid = request["ssid"] as? String, !ssid.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      promise.reject("ERR_BLE_REQUEST", "SSID is required.")
      return
    }
    guard pendingOperation == nil else {
      promise.reject("ERR_BLE_BUSY", "A BLE operation is already running.")
      return
    }

    let payload: [String: Any] = [
      "ssid": ssid,
      "password": request["password"] as? String ?? "",
      "interface": request["interfaceName"] as? String ?? "wlan0"
    ]

    do {
      let data = try JSONSerialization.data(withJSONObject: payload)
      connect(nativeId: nativeId, promise: promise, writePayload: data)
    } catch {
      promise.reject("ERR_BLE_PAYLOAD", "Unable to encode provisioning payload.")
    }
  }

  func readStatus(request: [String: Any], promise: Promise) {
    guard guardBluetoothReady(promise) else {
      return
    }
    guard let nativeId = request["nativeId"] as? String, !nativeId.isEmpty else {
      promise.reject("ERR_BLE_REQUEST", "nativeId is required.")
      return
    }
    guard pendingOperation == nil else {
      promise.reject("ERR_BLE_BUSY", "A BLE operation is already running.")
      return
    }
    connect(nativeId: nativeId, promise: promise, writePayload: nil)
  }

  func centralManagerDidUpdateState(_ central: CBCentralManager) {
    if central.state == .poweredOn, let timeoutMs = pendingScanTimeoutMs {
      pendingScanTimeoutMs = nil
      startBleScan(timeoutMs: timeoutMs)
      return
    }

    if central.state != .poweredOn && pendingScanTimeoutMs != nil {
      pendingScanTimeoutMs = nil
      rejectScan("ERR_BLE_UNAVAILABLE", bluetoothStateMessage())
    }
  }

  func centralManager(
    _ central: CBCentralManager,
    didDiscover peripheral: CBPeripheral,
    advertisementData: [String: Any],
    rssi RSSI: NSNumber
  ) {
    let nativeId = peripheral.identifier.uuidString
    peripherals[nativeId] = peripheral

    var item: [String: Any] = [
      "nativeId": nativeId,
      "name": (advertisementData[CBAdvertisementDataLocalNameKey] as? String) ?? peripheral.name as Any,
      "rssi": RSSI.intValue
    ]

    if let serviceData = advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data],
       let payload = serviceData[embeddedServiceUUID] {
      item.merge(parseEmbeddedPayload(payload)) { _, new in new }
    }

    scanResults[nativeId] = item.compactMapValues { value in
      if value is NSNull {
        return nil
      }
      return value
    }
  }

  func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
    peripheral.delegate = self
    peripheral.discoverServices([embeddedServiceUUID])
  }

  func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
    rejectOperation("ERR_BLE_CONNECT", error?.localizedDescription ?? "Unable to connect to embedded device.")
  }

  func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
    if let error {
      rejectOperation("ERR_BLE_SERVICES", error.localizedDescription)
      return
    }
    guard let service = peripheral.services?.first(where: { $0.uuid == embeddedServiceUUID }) else {
      rejectOperation("ERR_BLE_SERVICES", "Alia provisioning service was not found.")
      return
    }
    peripheral.discoverCharacteristics([networkStatusUUID, networkConfigUUID], for: service)
  }

  func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
    if let error {
      rejectOperation("ERR_BLE_CHARACTERISTICS", error.localizedDescription)
      return
    }
    guard let operation = pendingOperation else {
      return
    }

    if let payload = operation.writePayload {
      guard let characteristic = service.characteristics?.first(where: { $0.uuid == networkConfigUUID }) else {
        rejectOperation("ERR_BLE_CHARACTERISTICS", "Alia network config characteristic was not found.")
        return
      }
      peripheral.writeValue(payload, for: characteristic, type: .withResponse)
    } else {
      guard let characteristic = service.characteristics?.first(where: { $0.uuid == networkStatusUUID }) else {
        rejectOperation("ERR_BLE_CHARACTERISTICS", "Alia network status characteristic was not found.")
        return
      }
      peripheral.readValue(for: characteristic)
    }
  }

  func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
    if let error {
      rejectOperation("ERR_BLE_WRITE", error.localizedDescription)
      return
    }
    resolveOperation(nil)
  }

  func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
    if let error {
      rejectOperation("ERR_BLE_READ", error.localizedDescription)
      return
    }
    guard let data = characteristic.value else {
      resolveOperation(defaultNetworkStatus(message: "No status payload returned."))
      return
    }

    do {
      let value = try JSONSerialization.jsonObject(with: data)
      if let status = value as? [String: Any] {
        resolveOperation(status)
      } else {
        resolveOperation(defaultNetworkStatus(message: "Invalid status payload returned."))
      }
    } catch {
      resolveOperation(defaultNetworkStatus(message: "Unable to parse status payload."))
    }
  }

  private func connect(nativeId: String, promise: Promise, writePayload: Data?) {
    guard central.state == .poweredOn else {
      promise.reject("ERR_BLE_UNAVAILABLE", bluetoothStateMessage())
      return
    }
    guard let peripheral = resolvePeripheral(nativeId) else {
      promise.reject("ERR_BLE_DEVICE", "Embedded device is not available. Scan again and retry.")
      return
    }
    pendingOperation = PendingOperation(peripheral: peripheral, promise: promise, writePayload: writePayload)
    peripherals[nativeId] = peripheral
    peripheral.delegate = self
    central.connect(peripheral)
  }

  private func resolvePeripheral(_ nativeId: String) -> CBPeripheral? {
    if let peripheral = peripherals[nativeId] {
      return peripheral
    }
    guard let uuid = UUID(uuidString: nativeId) else {
      return nil
    }
    return central.retrievePeripherals(withIdentifiers: [uuid]).first
  }

  private func resolveOperation(_ value: Any?) {
    guard let operation = pendingOperation else {
      return
    }
    central.cancelPeripheralConnection(operation.peripheral)
    operation.promise.resolve(value)
    pendingOperation = nil
  }

  private func rejectOperation(_ code: String, _ message: String) {
    guard let operation = pendingOperation else {
      return
    }
    central.cancelPeripheralConnection(operation.peripheral)
    operation.promise.reject(code, message)
    pendingOperation = nil
  }

  private func bluetoothStateMessage() -> String {
    switch central.state {
    case .poweredOn:
      return "Bluetooth is available."
    case .poweredOff:
      return "Bluetooth is powered off."
    case .unauthorized:
      return "Bluetooth permission is not granted."
    case .unsupported:
      return "Bluetooth LE is not supported on this device."
    default:
      return "Bluetooth is not ready yet."
    }
  }
}

private struct PendingOperation {
  let peripheral: CBPeripheral
  let promise: Promise
  let writePayload: Data?
}

private func parseEmbeddedPayload(_ data: Data) -> [String: Any] {
  guard data.count >= 12 else {
    return ["status": "unknown"]
  }

  let bytes = [UInt8](data)
  let port = Int(UInt16(bytes[2]) << 8 | UInt16(bytes[3]))
  let serverFingerprint = hex(bytes[8..<12])
  var result: [String: Any] = [
    "status": embeddedStatus(bytes[1]),
    "port": port,
    "deviceFingerprint": hex(bytes[4..<8])
  ]
  if serverFingerprint != "00000000" {
    result["serverFingerprint"] = serverFingerprint
  }
  return result
}

private func embeddedStatus(_ code: UInt8) -> String {
  switch code {
  case 1:
    return "bound"
  case 2:
    return "handoff_pending"
  case 0:
    return "unbound"
  default:
    return "unknown"
  }
}

private func hex(_ bytes: ArraySlice<UInt8>) -> String {
  bytes.map { String(format: "%02x", $0) }.joined()
}

private func defaultNetworkStatus(message: String) -> [String: Any] {
  [
    "schema": "alia.embedded.network_provisioning.v1",
    "state": "failed",
    "message": message
  ]
}
