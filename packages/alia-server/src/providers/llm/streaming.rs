//! Shared Server-Sent Events helpers for LLM adapters.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
    event: Option<String>,
    data_lines: Vec<String>,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();

        while let Some(line_end) = self.buffer.find('\n') {
            let mut line = self.buffer[..line_end].to_string();
            self.buffer = self.buffer[line_end + 1..].to_string();
            if let Some(stripped) = line.strip_suffix('\r') {
                line = stripped.to_string();
            }
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(SseEvent {
                        event: self.event.take(),
                        data: self.data_lines.join("\n"),
                    });
                    self.data_lines.clear();
                } else {
                    self.event = None;
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                self.event = Some(value.trim_start().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data_lines.push(value.trim_start().to_string());
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_split_sse_event() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"event: content\ndata: Hel").is_empty());
        let events = decoder.push(b"lo\n\n");
        assert_eq!(
            events,
            vec![SseEvent {
                event: Some("content".to_string()),
                data: "Hello".to_string(),
            }]
        );
    }

    #[test]
    fn joins_multiline_data() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b"data: one\ndata: two\n\n");
        assert_eq!(events[0].data, "one\ntwo");
    }
}
