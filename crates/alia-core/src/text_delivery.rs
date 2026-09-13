//! Text evidence is measured in source UTF-8 bytes. A terminal write says
//! nothing about whether a human read, understood, or even saw the text.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::delivery::ObservationEnd;

pub const MAX_TEXT_CHUNK_BYTES: usize = 8 * 1024;
pub const MAX_TEXT_BYTES: u64 = 256 * 1024;
pub const MAX_TEXT_CHUNKS: u64 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextIdentity {
    pub text_id: Uuid,
    pub channel_instance_id: Uuid,
}
impl TextIdentity {
    pub fn is_valid(self) -> bool {
        !self.text_id.is_nil() && !self.channel_instance_id.is_nil()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChunk {
    /// Zero-based sequence, independent of model token boundaries.
    pub sequence: u64,
    pub utf8_offset: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TextFrame {
    Chunk(TextChunk),
    /// EOF without this explicit producer commitment is not successful End.
    End {
        chunks: u64,
        utf8_bytes: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextWriteBoundary {
    StdoutWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextProgress {
    pub identity: TextIdentity,
    pub receipt_sequence: u64,
    pub accepted_utf8_bytes: u64,
    /// A known contiguous prefix; it may end inside a UTF-8 character after a
    /// partial OS write. Unknown does not imply unwritten remainder.
    pub written_utf8_bytes: Option<u64>,
    pub write_boundary: Option<TextWriteBoundary>,
    pub end_accepted: bool,
    pub observation_end: Option<ObservationEnd>,
}

impl TextProgress {
    pub fn validate(
        &self,
        previous: Option<&Self>,
        offered: u64,
        end_offered: bool,
    ) -> Result<(), &'static str> {
        if !self.identity.is_valid() || self.receipt_sequence == 0 {
            return Err("invalid text identity or receipt sequence");
        }
        if offered > MAX_TEXT_BYTES || self.accepted_utf8_bytes > offered {
            return Err("text acceptance exceeds durable offer");
        }
        if self
            .written_utf8_bytes
            .is_some_and(|n| n > self.accepted_utf8_bytes)
        {
            return Err("text write exceeds acceptance");
        }
        if self.written_utf8_bytes.is_some() != self.write_boundary.is_some() {
            return Err("text write has no actual boundary");
        }
        if self.end_accepted && (!end_offered || self.accepted_utf8_bytes != offered) {
            return Err("text End lacks complete producer offer");
        }
        if let Some(old) = previous {
            if self.identity != old.identity
                || self.receipt_sequence <= old.receipt_sequence
                || old.observation_end.is_some()
            {
                return Err("text receipt identity, sequence or terminal state changed");
            }
            if self.accepted_utf8_bytes < old.accepted_utf8_bytes
                || old
                    .written_utf8_bytes
                    .is_some_and(|n| self.written_utf8_bytes.is_none_or(|next| next < n))
            {
                return Err("text prefix regressed");
            }
            if old.write_boundary.is_some() && old.write_boundary != self.write_boundary {
                return Err("text write boundary changed");
            }
            if old.end_accepted
                && (!self.end_accepted || old.accepted_utf8_bytes != self.accepted_utf8_bytes)
            {
                return Err("text changed after End acceptance");
            }
        }
        match self.observation_end {
            Some(ObservationEnd::SubmissionFinished)
                if !self.end_accepted || self.written_utf8_bytes != Some(offered) =>
            {
                return Err("text Written lacks complete write and End evidence");
            }
            Some(ObservationEnd::PresentedFinished) => {
                return Err("text transport cannot prove a human read it");
            }
            Some(ObservationEnd::Rejected { .. }) if self.accepted_utf8_bytes != 0 => {
                return Err("text rejected after acceptance");
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_utf8_write_is_a_prefix_not_read_or_complete_evidence() {
        let mut p = TextProgress {
            identity: TextIdentity {
                text_id: Uuid::new_v4(),
                channel_instance_id: Uuid::new_v4(),
            },
            receipt_sequence: 1,
            accepted_utf8_bytes: 3,
            written_utf8_bytes: Some(1),
            write_boundary: Some(TextWriteBoundary::StdoutWrite),
            end_accepted: false,
            observation_end: Some(ObservationEnd::Unknown {
                reason: "write failed inside a UTF-8 character".into(),
            }),
        };
        assert!(p.validate(None, 3, false).is_ok());
        p.observation_end = Some(ObservationEnd::SubmissionFinished);
        assert!(p.validate(None, 3, false).is_err());
        p.end_accepted = true;
        p.written_utf8_bytes = Some(3);
        assert!(p.validate(None, 3, true).is_ok());
        p.observation_end = Some(ObservationEnd::PresentedFinished);
        assert!(p.validate(None, 3, true).is_err());
    }
}
