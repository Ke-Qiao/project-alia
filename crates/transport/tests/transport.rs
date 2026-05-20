use std::time::Duration;

use alia_transport::{
    DEFAULT_EMBEDDED_DISCOVERY_SERVICE, EMBEDDED_INGRESS_SCOPE, bind_payload,
    embedded_device_auth_subject, mint_auth_token, normalize_service_name, sign_payload,
    verify_auth_token, verify_payload,
};
use ed25519_dalek::SigningKey;

#[test]
fn normalizes_embedded_service_name() {
    assert_eq!(
        normalize_service_name("_alia-embedded._tcp"),
        "_alia-embedded._tcp.local."
    );
    assert_eq!(
        normalize_service_name(DEFAULT_EMBEDDED_DISCOVERY_SERVICE),
        "_alia-embedded._tcp.local."
    );
}

#[test]
fn device_ingress_auth_subject_falls_back_to_public_key() {
    let secret: [u8; 32] = rand::random();
    let signing_key = SigningKey::from_bytes(&secret);
    let public_key = signing_key.verifying_key().to_bytes();
    let subject = embedded_device_auth_subject("", &public_key);

    let token = mint_auth_token(
        &signing_key,
        &subject,
        EMBEDDED_INGRESS_SCOPE,
        Duration::from_secs(900),
    )
    .unwrap();
    let claims = verify_auth_token(
        &token,
        &public_key,
        &subject,
        EMBEDDED_INGRESS_SCOPE,
        Duration::from_secs(300),
    )
    .unwrap();
    assert_eq!(claims.server_id, subject);
}

#[test]
fn signs_and_verifies_payload() {
    let secret: [u8; 32] = rand::random();
    let signing_key = SigningKey::from_bytes(&secret);
    let payload = bind_payload("server-1", "Server", "http://127.0.0.1:3000", &[], 42);
    let signature = sign_payload(&signing_key, &payload);
    let public_key = signing_key.verifying_key().to_bytes();
    verify_payload(&public_key, &payload, &signature).unwrap();
}

#[test]
fn mints_and_verifies_auth_token() {
    let secret: [u8; 32] = rand::random();
    let signing_key = SigningKey::from_bytes(&secret);
    let public_key = signing_key.verifying_key().to_bytes();
    let token = mint_auth_token(
        &signing_key,
        "server-main",
        "embedded:control",
        Duration::from_secs(900),
    )
    .unwrap();
    let claims = verify_auth_token(
        &token,
        &public_key,
        "server-main",
        "embedded:control",
        Duration::from_secs(300),
    )
    .unwrap();
    assert_eq!(claims.server_id, "server-main");
}
