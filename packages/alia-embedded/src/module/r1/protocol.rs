use serde_json::Value;

// Control UART only; the separate CDC raw-audio interface permits larger frames.
pub(super) const MAX_PAYLOAD: usize = 1024;
const HEADER_BYTES: usize = 7;
const MAX_KEYWORD_BYTES: usize = 256;
const HANDSHAKE_PAYLOAD: &[u8] = &[0xa5, 0, 0, 0];
pub(super) const MANUAL_CAPTURE_FIRMWARE: &str = "version: UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004; build date: Sep  3 2026 22:40:02";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Frame {
    pub kind: u8,
    pub id: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub(super) struct Wakeup {
    pub keyword: String,
    pub angle_deg: Option<f64>,
    pub beam: Option<u8>,
}

#[derive(Default)]
pub(super) struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        for &byte in bytes {
            self.buffer.push(byte);
            loop {
                let Some(start) = self.buffer.iter().position(|&byte| byte == 0xa5) else {
                    self.buffer.clear();
                    break;
                };
                self.buffer.drain(..start);
                if self.buffer.len() >= 2 && self.buffer[1] != 0x01 {
                    self.buffer.remove(0);
                    continue;
                }
                if self.buffer.len() < HEADER_BYTES {
                    break;
                }
                let length = u16::from_le_bytes([self.buffer[3], self.buffer[4]]) as usize;
                if length > MAX_PAYLOAD {
                    self.buffer.remove(0);
                    continue;
                }
                let total = HEADER_BYTES + length + 1;
                if self.buffer.len() < total {
                    break;
                }
                if sum(&self.buffer[..total]) != 0 {
                    self.buffer.remove(0);
                    continue;
                }
                frames.push(Frame {
                    kind: self.buffer[2],
                    id: u16::from_le_bytes([self.buffer[5], self.buffer[6]]),
                    payload: self.buffer[HEADER_BYTES..total - 1].to_vec(),
                });
                self.buffer.drain(..total);
            }
        }
        frames
    }
}

impl Frame {
    pub fn command_ack(&self, expected_id: u16) -> Option<bool> {
        (self.kind == 0x05 && self.id == expected_id).then(|| {
            serde_json::from_slice::<Value>(&self.payload)
                .ok()
                .and_then(|body| body.get("code").and_then(Value::as_i64))
                == Some(0)
        })
    }

    pub fn handshake_ack(&self) -> Option<Vec<u8>> {
        (self.kind == 0x01 && self.payload == HANDSHAKE_PAYLOAD)
            .then(|| encode(0xff, self.id, HANDSHAKE_PAYLOAD))
    }

    pub fn version_response(&self, expected_id: u16) -> Option<String> {
        if self.kind != 0x05 || self.id != expected_id {
            return None;
        }
        let body: Value = serde_json::from_slice(&self.payload).ok()?;
        if body.get("code")?.as_i64()? != 0 {
            return None;
        }
        let text = body.get("content")?.as_str()?.trim();
        if text.strip_prefix("version:")?.trim().is_empty() {
            return None;
        }
        Some(text.to_owned())
    }

    pub fn wakeup(&self) -> Option<Wakeup> {
        if self.kind != 0x04 {
            return None;
        }
        let body: Value = serde_json::from_slice(&self.payload).ok()?;
        if body.get("type")?.as_str()? != "aiui_event" {
            return None;
        }
        let content = object_field(Some(&body), "content")?;
        let info = object_field(content.as_ref(), "info")?;
        let ivw = object_field(info.as_ref(), "ivw")?;
        let mut keyword = None;
        let mut angle_deg = None;
        let mut beam = None;
        for layer in [Some(&body), content.as_ref(), info.as_ref(), ivw.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(event_type) = layer.get("eventType")
                && event_type.as_u64() != Some(4)
            {
                return None;
            }
            if let Some(value) = layer.get("keyword") {
                keyword = value
                    .as_str()
                    .filter(|text| !text.trim().is_empty() && text.len() <= MAX_KEYWORD_BYTES);
            }
            if let Some(value) = layer.get("angle") {
                angle_deg = value
                    .as_f64()
                    .filter(|angle| angle.is_finite() && (0.0..=360.0).contains(angle))
                    .map(|angle| if angle == 360.0 { 0.0 } else { angle });
            }
            if let Some(value) = layer.get("physical").or_else(|| layer.get("beam")) {
                beam = value
                    .as_f64()
                    .filter(|beam| (0.0..=5.0).contains(beam) && beam.fract() == 0.0)
                    .map(|beam| beam as u8);
            }
        }
        Some(Wakeup {
            keyword: keyword?.to_owned(),
            angle_deg,
            beam,
        })
    }
}

pub(super) fn version_query(id: u16) -> Vec<u8> {
    encode(0x05, id, br#"{"type": "version"}"#)
}

pub(super) fn manual_wakeup_beam_zero(id: u16) -> Vec<u8> {
    encode(
        0x05,
        id,
        br#"{"type":"manual_wakeup","content":{"beam":0}}"#,
    )
}

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

pub(super) fn encode(kind: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= MAX_PAYLOAD);
    let mut frame = vec![0xa5, 0x01, kind];
    frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    frame.extend_from_slice(&id.to_le_bytes());
    frame.extend_from_slice(payload);
    frame.push(sum(&frame).wrapping_neg());
    frame
}

// A present malformed nested object invalidates the event instead of falling back.
fn object_field(parent: Option<&Value>, key: &str) -> Option<Option<Value>> {
    let Some(value) = parent.and_then(|parent| parent.get(key)) else {
        return Some(None);
    };
    let value = match value {
        Value::Object(_) => value.clone(),
        Value::String(text) => serde_json::from_str(text).ok()?,
        _ => return None,
    };
    value.is_object().then_some(Some(value))
}

#[cfg(test)]
#[path = "protocol/tests.rs"]
mod tests;
