use alia_embedded::identity::{ensure_device_keypair, load_device_keypair};

#[test]
fn round_trips_device_keypair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.key");

    let created = ensure_device_keypair(&path).unwrap();
    let loaded = load_device_keypair(&path).unwrap();

    assert_eq!(created.public_key_bytes(), loaded.public_key_bytes());
}
