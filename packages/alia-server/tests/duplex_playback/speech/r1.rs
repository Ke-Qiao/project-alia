use super::*;
use alia_embedded::module::{lifecycle::ModuleHealth, r1::R1WakeWord};
use std::io::{ErrorKind, Read, Write};

pub(super) struct Board(std::fs::File);
impl Board {
    pub(super) fn new() -> (Self, Arc<R1WakeWord>) {
        let (master, wake) = alia_embedded::module::r1::test_pty_engine();
        (Self(master), Arc::new(wake))
    }
    pub(super) async fn verify(&mut self, wake: &R1WakeWord) {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut pending = Vec::new();
            while wake.health_check().await != ModuleHealth::Healthy {
                let mut bytes = [0; 256];
                match self.0.read(&mut bytes) {
                    Ok(count) => pending.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                    result => panic!("fixture UART version query: {result:?}"),
                }
                while pending.len() >= 7 {
                    assert_eq!(&pending[..3], &[0xa5, 1, 5]);
                    let length = u16::from_le_bytes([pending[3], pending[4]]) as usize + 8;
                    assert!(length <= 256);
                    if pending.len() < length {
                        break;
                    }
                    // The production host must only query version, never steer beam or force wake.
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(&pending[7..length - 1])
                            .unwrap(),
                        json!({"type":"version"})
                    );
                    let id = u16::from_le_bytes([pending[5], pending[6]]);
                    self.0
                        .write_all(&packet(
                            5,
                            id,
                            br#"{"code":0,"content":"version: UART_REAL, circle6:1.9.6.fixture"}"#,
                        ))
                        .unwrap();
                    pending.drain(..length);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    pub(super) fn wake(&mut self, id: u16) {
        let bytes = packet(4, id, br#"{"type":"aiui_event","content":{"info":"{\"ivw\":{\"keyword\":\"xiao3-fei1-xiao3-fei1\",\"angle\":344,\"physical\":0}}"}}"#);
        self.0.write_all(&bytes[..6]).unwrap();
        self.0.write_all(&bytes[6..]).unwrap();
    }

    pub(super) async fn reset_applied(&self, wake: &R1WakeWord) {
        eventually(|| async { wake.test_reset_applied() }).await;
    }
}
fn packet(kind: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xa5, 1, kind];
    bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes.push(0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))));
    bytes
}
