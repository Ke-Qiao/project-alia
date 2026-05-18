use std::collections::HashMap;

use alia_embedded::module::discovery::{encode_ble_payload, normalize_mdns_service_type};
use alia_embedded::module::traits::{DeviceInfo, DiscoveryMetadata};

#[test]
fn normalizes_mdns_service_type() {
    assert_eq!(
        normalize_mdns_service_type("_alia-embedded._tcp"),
        "_alia-embedded._tcp.local."
    );
    assert_eq!(
        normalize_mdns_service_type("_alia-embedded._tcp.local"),
        "_alia-embedded._tcp.local."
    );
}

#[test]
fn builds_compact_ble_payload() {
    let info = DeviceInfo {
        device_id: "alia-pi-a1b2c3d4".to_string(),
        device_name: "Living Room".to_string(),
        version: "0.1.0".to_string(),
        port: 50051,
        endpoints: Vec::new(),
        capabilities: Vec::new(),
        attributes: HashMap::new(),
    };
    let metadata = DiscoveryMetadata {
        status: "bound".to_string(),
        server_id: Some("server-main".to_string()),
        capabilities: Vec::new(),
        attributes: HashMap::new(),
    };

    let payload = encode_ble_payload(&info, &metadata);
    assert_eq!(payload.len(), 12);
    assert_eq!(payload[0], 1);
    assert_eq!(payload[1], 1);
}
