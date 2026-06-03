//! Cursor-based pagination (PRD §6.4).
//!
//! Wire shape: `base64(JSON({"version":1,"last_id":N,"last_created_at":"<RFC3339>"}))`.

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

use memlayer_core::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub version: u32,
    pub last_id: i64,
    pub last_created_at: String,
}

impl Cursor {
    pub fn new(last_id: i64, last_created_at: String) -> Self {
        Cursor {
            version: 1,
            last_id,
            last_created_at,
        }
    }

    /// Encode to the wire token.
    pub fn encode(&self) -> Result<String> {
        let json = serde_json::to_string(self)?;
        Ok(STANDARD_NO_PAD.encode(json.as_bytes()))
    }

    /// Decode an opaque wire token (EC-7: malformed → INVALID_ARGUMENT).
    pub fn decode(token: &str) -> Result<Self> {
        let bytes = STANDARD_NO_PAD
            .decode(token.as_bytes())
            .map_err(|e| Error::invalid(format!("malformed cursor: {e}")))?;
        let cursor: Cursor = serde_json::from_slice(&bytes)
            .map_err(|e| Error::invalid(format!("malformed cursor JSON: {e}")))?;
        if cursor.version != 1 {
            return Err(Error::invalid(format!(
                "unsupported cursor version: {}",
                cursor.version
            )));
        }
        Ok(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let c = Cursor::new(42, "2026-01-01T00:00:00+00:00".into());
        let s = c.encode().unwrap();
        let back = Cursor::decode(&s).unwrap();
        assert_eq!(back.last_id, 42);
        assert_eq!(back.last_created_at, "2026-01-01T00:00:00+00:00");
    }

    #[test]
    fn malformed_base64_rejected() {
        assert!(Cursor::decode("@@@").is_err());
    }

    #[test]
    fn malformed_json_rejected() {
        let s = STANDARD_NO_PAD.encode(b"not json");
        assert!(Cursor::decode(&s).is_err());
    }

    #[test]
    fn version_mismatch_rejected() {
        let c = Cursor {
            version: 99,
            last_id: 0,
            last_created_at: "2026-01-01T00:00:00+00:00".into(),
        };
        let s = c.encode().unwrap();
        assert!(Cursor::decode(&s).is_err());
    }
}
