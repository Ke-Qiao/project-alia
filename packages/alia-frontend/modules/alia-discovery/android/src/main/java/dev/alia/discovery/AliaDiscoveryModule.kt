package dev.alia.discovery

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import expo.modules.interfaces.permissions.PermissionsResponseListener
import expo.modules.interfaces.permissions.PermissionsStatus
import expo.modules.kotlin.Promise
import expo.modules.kotlin.exception.Exceptions
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import org.json.JSONObject
import java.nio.charset.StandardCharsets
import java.util.UUID

private const val ALIA_SERVER_SERVICE_TYPE = "_alia-server._tcp."
private const val EMBEDDED_SERVICE_UUID = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f1"
private const val NETWORK_STATUS_UUID = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f2"
private const val NETWORK_CONFIG_UUID = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f3"

class AliaDiscoveryModule : Module() {
  private val context: Context
    get() = appContext.reactContext ?: throw Exceptions.ReactContextLost()

  private val mainHandler = Handler(Looper.getMainLooper())
  private val serverScans = mutableSetOf<ServerScanner>()
  private val bleDiscovery by lazy { BleDiscovery(context, mainHandler) }

  override fun definition() = ModuleDefinition {
    Name("AliaDiscovery")

    AsyncFunction("scanServers") { options: Map<String, Any?>?, promise: Promise ->
      val scan = ServerScanner(
        context = context,
        timeoutMs = timeoutMs(options, 3500),
        promise = promise,
        mainHandler = mainHandler
      ) { finished ->
        serverScans.remove(finished)
      }
      serverScans.add(scan)
      scan.start()
    }

    AsyncFunction("scanEmbedded") { options: Map<String, Any?>?, promise: Promise ->
      ensureBlePermissions(promise) {
        bleDiscovery.scanEmbedded(timeoutMs(options, 5000), promise)
      }
    }

    AsyncFunction("provisionEmbeddedNetwork") { request: Map<String, Any?>, promise: Promise ->
      ensureBlePermissions(promise) {
        bleDiscovery.provision(request, promise)
      }
    }

    AsyncFunction("readEmbeddedNetworkStatus") { request: Map<String, Any?>, promise: Promise ->
      ensureBlePermissions(promise) {
        bleDiscovery.readStatus(request, promise)
      }
    }
  }

  private fun timeoutMs(options: Map<String, Any?>?, fallback: Int): Int {
    val value = options?.get("timeoutMs")
    return when (value) {
      is Number -> value.toInt()
      else -> fallback
    }.coerceAtLeast(500)
  }

  private fun ensureBlePermissions(promise: Promise, action: () -> Unit) {
    val permissions = requiredBlePermissions()
    if (permissions.isEmpty()) {
      action()
      return
    }

    val permissionsManager = appContext.permissions
    if (permissionsManager == null) {
      promise.reject("ERR_BLE_PERMISSION", "Permissions module is unavailable.", null)
      return
    }

    if (permissionsManager.hasGrantedPermissions(*permissions.toTypedArray())) {
      action()
      return
    }

    permissionsManager.askForPermissions(
      PermissionsResponseListener { response ->
        val granted = response.values.all { it.status == PermissionsStatus.GRANTED }
        if (granted) {
          action()
        } else {
          promise.reject("ERR_BLE_PERMISSION", "Bluetooth permission is required to discover and configure nearby Alia devices.", null)
        }
      },
      *permissions.toTypedArray()
    )
  }

  private fun requiredBlePermissions(): List<String> {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      listOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
    } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
      listOf(Manifest.permission.ACCESS_FINE_LOCATION)
    } else {
      emptyList()
    }
  }
}

private class ServerScanner(
  private val context: Context,
  private val timeoutMs: Int,
  private val promise: Promise,
  private val mainHandler: Handler,
  private val onFinish: (ServerScanner) -> Unit
) {
  private val nsdManager = context.getSystemService(Context.NSD_SERVICE) as NsdManager
  private val results = linkedMapOf<String, Map<String, Any?>>()
  private var finished = false
  private var multicastLock: WifiManager.MulticastLock? = null

  private val discoveryListener = object : NsdManager.DiscoveryListener {
    override fun onDiscoveryStarted(serviceType: String) = Unit

    override fun onServiceFound(serviceInfo: NsdServiceInfo) {
      if (serviceInfo.serviceType == ALIA_SERVER_SERVICE_TYPE) {
        resolve(serviceInfo)
      }
    }

    override fun onServiceLost(serviceInfo: NsdServiceInfo) = Unit

    override fun onDiscoveryStopped(serviceType: String) = Unit

    override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
      reject("ERR_MDNS_SCAN", "Unable to start local Alia Server discovery: $errorCode")
    }

    override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit
  }

  fun start() {
    acquireMulticastLock()
    try {
      nsdManager.discoverServices(ALIA_SERVER_SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discoveryListener)
      mainHandler.postDelayed({ finish() }, timeoutMs.toLong())
    } catch (error: Throwable) {
      reject("ERR_MDNS_SCAN", error.message ?: "Unable to scan for Alia Server.")
    }
  }

  private fun resolve(serviceInfo: NsdServiceInfo) {
    val listener = object : NsdManager.ResolveListener {
      override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
        record(serviceInfo)
      }

      override fun onServiceResolved(resolved: NsdServiceInfo) {
        record(resolved)
      }
    }

    try {
      nsdManager.resolveService(serviceInfo, listener)
    } catch (_: Throwable) {
      record(serviceInfo)
    }
  }

  private fun record(serviceInfo: NsdServiceInfo) {
    val txt = txtAttributes(serviceInfo)
    val host = serviceInfo.host?.hostAddress ?: serviceInfo.host?.hostName
    val port = serviceInfo.port.takeIf { it > 0 }
    val endpoint = txt["endpoint"] ?: if (host != null && port != null) "http://$host:$port" else null
    if (endpoint == null) {
      return
    }

    val id = "${serviceInfo.serviceName}|$endpoint"
    results[id] = mapOf(
      "id" to id,
      "name" to serviceInfo.serviceName,
      "endpoint" to endpoint,
      "host" to host,
      "port" to port,
      "mode" to txt["mode"],
      "txt" to txt
    ).filterValues { it != null }
  }

  private fun txtAttributes(serviceInfo: NsdServiceInfo): Map<String, String> {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
      serviceInfo.attributes.mapValues { (_, value) -> String(value, StandardCharsets.UTF_8) }
    } else {
      emptyMap()
    }
  }

  private fun acquireMulticastLock() {
    val wifiManager = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
    multicastLock = wifiManager?.createMulticastLock("alia-discovery-mdns")?.apply {
      setReferenceCounted(false)
      try {
        acquire()
      } catch (_: Throwable) {
      }
    }
  }

  private fun finish() {
    if (finished) {
      return
    }
    finished = true
    try {
      nsdManager.stopServiceDiscovery(discoveryListener)
    } catch (_: Throwable) {
    }
    releaseMulticastLock()
    promise.resolve(results.values.toList())
    onFinish(this)
  }

  private fun reject(code: String, message: String) {
    if (finished) {
      return
    }
    finished = true
    releaseMulticastLock()
    promise.reject(code, message, null)
    onFinish(this)
  }

  private fun releaseMulticastLock() {
    try {
      multicastLock?.release()
    } catch (_: Throwable) {
    }
    multicastLock = null
  }
}

private class BleDiscovery(
  private val context: Context,
  private val mainHandler: Handler
) {
  private val serviceUuid = UUID.fromString(EMBEDDED_SERVICE_UUID)
  private val serviceParcelUuid = ParcelUuid(serviceUuid)
  private val statusUuid = UUID.fromString(NETWORK_STATUS_UUID)
  private val configUuid = UUID.fromString(NETWORK_CONFIG_UUID)
  private val bluetoothManager = context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
  private val bluetoothAdapter: BluetoothAdapter?
    get() = bluetoothManager.adapter

  private val devices = mutableMapOf<String, android.bluetooth.BluetoothDevice>()
  private var scanPromise: Promise? = null
  private var scanResults = linkedMapOf<String, Map<String, Any?>>()
  private var scanCallback: ScanCallback? = null
  private var pending: PendingGatt? = null

  @SuppressLint("MissingPermission")
  fun scanEmbedded(timeoutMs: Int, promise: Promise) {
    if (scanPromise != null) {
      promise.reject("ERR_BLE_BUSY", "A BLE scan is already running.", null)
      return
    }

    val scanner = bluetoothAdapter?.bluetoothLeScanner
    if (scanner == null || bluetoothAdapter?.isEnabled != true) {
      promise.reject("ERR_BLE_UNAVAILABLE", "Bluetooth LE is unavailable or powered off.", null)
      return
    }

    scanPromise = promise
    scanResults = linkedMapOf()
    val callback = object : ScanCallback() {
      override fun onScanResult(callbackType: Int, result: ScanResult) {
        record(result)
      }

      override fun onBatchScanResults(results: MutableList<ScanResult>) {
        results.forEach { record(it) }
      }

      override fun onScanFailed(errorCode: Int) {
        rejectScan("ERR_BLE_SCAN", "BLE scan failed: $errorCode")
      }
    }
    scanCallback = callback

    val filters = listOf(ScanFilter.Builder().setServiceUuid(serviceParcelUuid).build())
    val settings = ScanSettings.Builder()
      .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
      .build()

    try {
      scanner.startScan(filters, settings, callback)
      mainHandler.postDelayed({ finishScan() }, timeoutMs.toLong())
    } catch (error: Throwable) {
      rejectScan("ERR_BLE_SCAN", error.message ?: "Unable to start BLE scan.")
    }
  }

  fun provision(request: Map<String, Any?>, promise: Promise) {
    val nativeId = request["nativeId"] as? String
    val ssid = request["ssid"] as? String
    if (nativeId.isNullOrBlank()) {
      promise.reject("ERR_BLE_REQUEST", "nativeId is required.", null)
      return
    }
    if (ssid.isNullOrBlank()) {
      promise.reject("ERR_BLE_REQUEST", "SSID is required.", null)
      return
    }

    val payload = JSONObject()
      .put("ssid", ssid)
      .put("password", request["password"] as? String ?: "")
      .put("interface", request["interfaceName"] as? String ?: "wlan0")
      .toString()
      .toByteArray(StandardCharsets.UTF_8)

    connect(nativeId, promise, payload)
  }

  fun readStatus(request: Map<String, Any?>, promise: Promise) {
    val nativeId = request["nativeId"] as? String
    if (nativeId.isNullOrBlank()) {
      promise.reject("ERR_BLE_REQUEST", "nativeId is required.", null)
      return
    }
    connect(nativeId, promise, null)
  }

  @SuppressLint("MissingPermission")
  private fun connect(nativeId: String, promise: Promise, payload: ByteArray?) {
    if (pending != null) {
      promise.reject("ERR_BLE_BUSY", "A BLE operation is already running.", null)
      return
    }

    val adapter = bluetoothAdapter
    if (adapter?.isEnabled != true) {
      promise.reject("ERR_BLE_UNAVAILABLE", "Bluetooth LE is unavailable or powered off.", null)
      return
    }

    val device = try {
      devices[nativeId] ?: adapter.getRemoteDevice(nativeId)
    } catch (_: Throwable) {
      null
    }
    if (device == null) {
      promise.reject("ERR_BLE_DEVICE", "Embedded device is not available. Scan again and retry.", null)
      return
    }

    val callback = gattCallback()
    pending = PendingGatt(promise = promise, writePayload = payload)
    try {
      pending?.gatt = device.connectGatt(context, false, callback)
    } catch (error: Throwable) {
      rejectOperation("ERR_BLE_CONNECT", error.message ?: "Unable to connect to embedded device.")
    }
  }

  @SuppressLint("MissingPermission")
  private fun record(result: ScanResult) {
    val device = result.device
    val nativeId = device.address
    devices[nativeId] = device

    val item = mutableMapOf<String, Any?>(
      "nativeId" to nativeId,
      "name" to (result.scanRecord?.deviceName ?: safeDeviceName(device)),
      "rssi" to result.rssi
    )

    result.scanRecord?.getServiceData(serviceParcelUuid)?.let { payload ->
      item.putAll(parseEmbeddedPayload(payload))
    }

    scanResults[nativeId] = item.filterValues { it != null }
  }

  @SuppressLint("MissingPermission")
  private fun safeDeviceName(device: android.bluetooth.BluetoothDevice): String? {
    return try {
      device.name
    } catch (_: SecurityException) {
      null
    }
  }

  @SuppressLint("MissingPermission")
  private fun finishScan() {
    val promise = scanPromise ?: return
    try {
      scanCallback?.let { bluetoothAdapter?.bluetoothLeScanner?.stopScan(it) }
    } catch (_: Throwable) {
    }
    scanCallback = null
    scanPromise = null
    promise.resolve(scanResults.values.toList())
  }

  @SuppressLint("MissingPermission")
  private fun rejectScan(code: String, message: String) {
    val promise = scanPromise ?: return
    try {
      scanCallback?.let { bluetoothAdapter?.bluetoothLeScanner?.stopScan(it) }
    } catch (_: Throwable) {
    }
    scanCallback = null
    scanPromise = null
    promise.reject(code, message, null)
  }

  private fun gattCallback(): BluetoothGattCallback {
    return object : BluetoothGattCallback() {
      @SuppressLint("MissingPermission")
      override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
        if (status != BluetoothGatt.GATT_SUCCESS) {
          rejectOperation("ERR_BLE_CONNECT", "GATT connection failed: $status")
          return
        }

        if (newState == BluetoothProfile.STATE_CONNECTED) {
          gatt.discoverServices()
        } else if (newState == BluetoothProfile.STATE_DISCONNECTED && pending != null) {
          rejectOperation("ERR_BLE_CONNECT", "GATT connection was disconnected.")
        }
      }

      override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
        if (status != BluetoothGatt.GATT_SUCCESS) {
          rejectOperation("ERR_BLE_SERVICES", "GATT service discovery failed: $status")
          return
        }

        val service = gatt.getService(serviceUuid)
        if (service == null) {
          rejectOperation("ERR_BLE_SERVICES", "Alia provisioning service was not found.")
          return
        }

        val current = pending ?: return
        if (current.writePayload != null) {
          writeConfig(gatt, service, current.writePayload)
        } else {
          readStatus(gatt, service)
        }
      }

      override fun onCharacteristicWrite(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        status: Int
      ) {
        if (status == BluetoothGatt.GATT_SUCCESS) {
          resolveOperation(null)
        } else {
          rejectOperation("ERR_BLE_WRITE", "GATT write failed: $status")
        }
      }

      @Deprecated("Deprecated in Android 13 but still called by older platform versions.")
      override fun onCharacteristicRead(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        status: Int
      ) {
        handleStatusRead(characteristic.value, status)
      }

      override fun onCharacteristicRead(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray,
        status: Int
      ) {
        handleStatusRead(value, status)
      }
    }
  }

  @SuppressLint("MissingPermission")
  private fun writeConfig(gatt: BluetoothGatt, service: BluetoothGattService, payload: ByteArray) {
    val characteristic = service.getCharacteristic(configUuid)
    if (characteristic == null) {
      rejectOperation("ERR_BLE_CHARACTERISTICS", "Alia network config characteristic was not found.")
      return
    }

    characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
    @Suppress("DEPRECATION")
    characteristic.value = payload
    @Suppress("DEPRECATION")
    val started = gatt.writeCharacteristic(characteristic)
    if (!started) {
      rejectOperation("ERR_BLE_WRITE", "Unable to start GATT write.")
    }
  }

  @SuppressLint("MissingPermission")
  private fun readStatus(gatt: BluetoothGatt, service: BluetoothGattService) {
    val characteristic = service.getCharacteristic(statusUuid)
    if (characteristic == null) {
      rejectOperation("ERR_BLE_CHARACTERISTICS", "Alia network status characteristic was not found.")
      return
    }

    if (!gatt.readCharacteristic(characteristic)) {
      rejectOperation("ERR_BLE_READ", "Unable to start GATT read.")
    }
  }

  private fun handleStatusRead(value: ByteArray?, status: Int) {
    if (status != BluetoothGatt.GATT_SUCCESS) {
      rejectOperation("ERR_BLE_READ", "GATT read failed: $status")
      return
    }
    if (value == null) {
      resolveOperation(defaultNetworkStatus("No status payload returned."))
      return
    }

    val text = String(value, StandardCharsets.UTF_8)
    try {
      val json = JSONObject(text)
      resolveOperation(jsonToMap(json))
    } catch (_: Throwable) {
      resolveOperation(defaultNetworkStatus("Unable to parse status payload."))
    }
  }

  @SuppressLint("MissingPermission")
  private fun resolveOperation(value: Any?) {
    val current = pending ?: return
    current.gatt?.disconnect()
    current.gatt?.close()
    current.promise.resolve(value)
    pending = null
  }

  @SuppressLint("MissingPermission")
  private fun rejectOperation(code: String, message: String) {
    val current = pending ?: return
    current.gatt?.disconnect()
    current.gatt?.close()
    current.promise.reject(code, message, null)
    pending = null
  }

  private fun parseEmbeddedPayload(payload: ByteArray): Map<String, Any?> {
    if (payload.size < 12) {
      return mapOf("status" to "unknown")
    }

    val port = ((payload[2].toInt() and 0xff) shl 8) or (payload[3].toInt() and 0xff)
    val serverFingerprint = payload.copyOfRange(8, 12).toHex()
    return mutableMapOf<String, Any?>(
      "status" to embeddedStatus(payload[1].toInt() and 0xff),
      "port" to port,
      "deviceFingerprint" to payload.copyOfRange(4, 8).toHex()
    ).apply {
      if (serverFingerprint != "00000000") {
        put("serverFingerprint", serverFingerprint)
      }
    }
  }

  private fun embeddedStatus(code: Int): String {
    return when (code) {
      1 -> "bound"
      2 -> "handoff_pending"
      0 -> "unbound"
      else -> "unknown"
    }
  }

  private fun defaultNetworkStatus(message: String): Map<String, Any?> {
    return mapOf(
      "schema" to "alia.embedded.network_provisioning.v1",
      "state" to "failed",
      "message" to message
    )
  }

  private fun jsonToMap(json: JSONObject): Map<String, Any?> {
    val map = mutableMapOf<String, Any?>()
    json.keys().forEach { key ->
      map[key] = json.opt(key)
    }
    return map
  }
}

private data class PendingGatt(
  val promise: Promise,
  val writePayload: ByteArray?,
  var gatt: BluetoothGatt? = null
)

private fun ByteArray.toHex(): String = joinToString(separator = "") { byte ->
  "%02x".format(byte.toInt() and 0xff)
}
