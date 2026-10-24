use std::time::Duration;

use alia_transport::{
    DEFAULT_EMBEDDED_DISCOVERY_SERVICE, EMBEDDED_INGRESS_SCOPE, bind_payload,
    embedded_device_auth_subject, mint_auth_token, normalize_service_name, sign_payload,
    verify_auth_token, verify_payload,
};
use ed25519_dalek::SigningKey;

#[test]
fn motion_v2_signature_binds_content_and_rejects_old_or_opposite_domains() {
    use alia_transport::{motion_reply_payload, motion_request_payload};
    let key = SigningKey::from_bytes(&[29; 32]);
    let public = key.verifying_key().to_bytes();
    let payload =
        br#"{"version":2,"asset":{"file":"reviewed.anim","version":1,"sha256":"abc","bytes":3}}"#;
    let request = motion_request_payload(payload);
    let signature = sign_payload(&key, &request);
    assert!(verify_payload(&public, &request, &signature).is_ok());
    assert!(verify_payload(&public, &motion_reply_payload(payload), &signature).is_err());
    let changed = String::from_utf8(payload.to_vec())
        .unwrap()
        .replace("abc", "def");
    assert!(
        verify_payload(
            &public,
            &motion_request_payload(changed.as_bytes()),
            &signature
        )
        .is_err()
    );
    // Neither the retired Body domains nor an earlier Motion version may pass.
    for domain in [
        b"alia-body-v1-request".as_slice(),
        b"alia-body-v2-request".as_slice(),
        b"alia-motion-v1-request".as_slice(),
    ] {
        let mut old_domain = Vec::new();
        for part in [domain, payload.as_slice()] {
            old_domain.extend_from_slice(&(part.len() as u32).to_be_bytes());
            old_domain.extend_from_slice(part);
        }
        let old_signature = sign_payload(&key, &old_domain);
        assert!(verify_payload(&public, &request, &old_signature).is_err());
    }
}

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
