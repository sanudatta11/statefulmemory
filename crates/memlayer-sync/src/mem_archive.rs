//! memlayer `.mem` archive: MLYR + MessagePack + zstd-19.
//! Default: XOR obfuscation. Optional: Argon2id + XChaCha20-Poly1305.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    Key, XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::error::{Result, SyncError};

pub const MAGIC: &[u8; 4] = b"MLYR";
pub const ARCHIVE_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 88;
pub const FLAG_ENCRYPTED: u8 = 0x01;
const ZSTD_LEVEL: i32 = 19;
const KEY_DOMAIN: &[u8] = b"memlayer.mem.v1\0";
const ARGON2_M_KIB: u32 = 64 * 1024;
const ARGON2_T: u32 = 3;
const ARGON2_P: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedObservation {
    pub id: i64,
    pub sync_id: String,
    pub session_id: String,
    pub r#type: String,
    pub title: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub scope: String,
    pub created_by: Option<String>,
    pub topic_key: Option<String>,
    pub normalized_hash: Option<String>,
    pub revision_count: i32,
    pub duplicate_count: i32,
    pub last_seen_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub review_after: Option<String>,
    pub code_anchor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedSession {
    pub id: String,
    pub directory: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedPrompt {
    pub id: i64,
    pub sync_id: String,
    pub session_id: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedFact {
    pub id: i64,
    pub obs_id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub temporal: Option<String>,
    pub salience: f64,
    pub superseded_by: Option<i64>,
    pub extracted_by: String,
    pub extracted_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedRelation {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub relation_type: String,
    pub confidence: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivePayload {
    pub format: String,
    pub archive_version: u16,
    pub schema_version: i32,
    pub exported_at: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<ArchivedObservation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<ArchivedSession>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<ArchivedPrompt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<ArchivedFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ArchivedRelation>,
}

pub struct EncodeParams {
    pub salt: [u8; 16],
    pub aead_nonce: [u8; 24],
    pub seed: Option<String>,
}

pub fn normalize_seed(seed: &str) -> Result<String> {
    let n: String = seed.nfc().collect();
    let n = n.trim().trim_end_matches(['\n', '\r']).to_string();
    if n.chars().count() < 12 {
        return Err(SyncError::SeedTooShort);
    }
    Ok(n)
}

fn derive_key(seed: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(32))
        .map_err(|e| SyncError::Compress(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(seed.as_bytes(), salt, &mut out[..])
        .map_err(|_| SyncError::InvalidSeed)?;
    Ok(out)
}

pub fn keystream_xor(data: &[u8], salt: &[u8; 16]) -> Vec<u8> {
    let mut key = Sha256::new();
    key.update(KEY_DOMAIN);
    key.update(salt);
    let key = key.finalize();
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0u64;
    while out.len() < data.len() {
        let mut h = Sha256::new();
        h.update(key);
        h.update(i.to_le_bytes());
        let block = h.finalize();
        for b in block {
            if out.len() == data.len() {
                break;
            }
            out.push(data[out.len()] ^ b);
        }
        i += 1;
    }
    out
}

fn pack_header(
    flags: u8,
    inner_len: u64,
    checksum: &[u8],
    salt: &[u8; 16],
    aead_nonce: &[u8; 24],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&ARCHIVE_VERSION.to_le_bytes());
    out.push(flags);
    out.push(0);
    out.extend_from_slice(&inner_len.to_le_bytes());
    out.extend_from_slice(checksum);
    out.extend_from_slice(salt);
    out.extend_from_slice(aead_nonce);
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

pub fn is_encrypted(bytes: &[u8]) -> Result<bool> {
    if bytes.len() < HEADER_LEN {
        return Err(SyncError::Truncated);
    }
    if &bytes[..4] != MAGIC {
        return Err(SyncError::NotArchive);
    }
    Ok(bytes[6] & FLAG_ENCRYPTED != 0)
}

pub fn encode(payload: &ArchivePayload, params: &EncodeParams) -> Result<Vec<u8>> {
    let inner = rmp_serde::to_vec(payload).map_err(|e| SyncError::Compress(e.to_string()))?;
    let checksum = Sha256::digest(&inner);
    let compressed = zstd::bulk::compress(&inner, ZSTD_LEVEL)
        .map_err(|e| SyncError::Compress(e.to_string()))?;
    let mut flags = 0u8;
    let body = if let Some(raw) = &params.seed {
        flags |= FLAG_ENCRYPTED;
        let seed = normalize_seed(raw)?;
        let key = derive_key(&seed, &params.salt)?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key[..]));
        cipher
            .encrypt(XNonce::from_slice(&params.aead_nonce), compressed.as_ref())
            .map_err(|_| SyncError::InvalidSeed)?
    } else {
        keystream_xor(&compressed, &params.salt)
    };
    let mut out = pack_header(
        flags,
        inner.len() as u64,
        &checksum,
        &params.salt,
        &params.aead_nonce,
    );
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode(bytes: &[u8], seed: Option<&str>) -> Result<ArchivePayload> {
    if bytes.len() < HEADER_LEN {
        return Err(SyncError::Truncated);
    }
    if &bytes[..4] != MAGIC {
        return Err(SyncError::NotArchive);
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != ARCHIVE_VERSION {
        return Err(SyncError::UnsupportedVersion(version));
    }
    let encrypted = bytes[6] & FLAG_ENCRYPTED != 0;
    let uncompressed_len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let want_sum = &bytes[16..48];
    let salt: [u8; 16] = bytes[48..64].try_into().unwrap();
    let aead_nonce: [u8; 24] = bytes[64..88].try_into().unwrap();
    let body = &bytes[88..];
    let compressed = if encrypted {
        let Some(raw) = seed else {
            return Err(SyncError::SeedRequired);
        };
        let seed = normalize_seed(raw)?;
        let key = derive_key(&seed, &salt)?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key[..]));
        cipher
            .decrypt(XNonce::from_slice(&aead_nonce), body)
            .map_err(|_| SyncError::InvalidSeed)?
    } else {
        keystream_xor(body, &salt)
    };
    let inner = zstd::decode_all(&compressed[..]).map_err(|e| SyncError::Decompress(e.to_string()))?;
    if inner.len() != uncompressed_len {
        return Err(SyncError::CorruptChunk("length mismatch".into()));
    }
    let got = Sha256::digest(&inner);
    if got.as_slice() != want_sum {
        return Err(SyncError::ChecksumMismatch);
    }
    rmp_serde::from_slice(&inner).map_err(|e| SyncError::Compress(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ArchivePayload {
        ArchivePayload {
            format: "memlayer.archive".into(),
            archive_version: 1,
            schema_version: 8,
            exported_at: "2026-09-12T00:00:00Z".into(),
            project: "demo".into(),
            observations: vec![],
            sessions: vec![],
            prompts: vec![],
            facts: vec![],
            relations: vec![],
        }
    }

    fn params(seed: Option<&str>) -> EncodeParams {
        EncodeParams {
            salt: [7u8; 16],
            aead_nonce: [9u8; 24],
            seed: seed.map(|s| s.to_string()),
        }
    }

    #[test]
    fn round_trip_default() {
        let bytes = encode(&sample(), &params(None)).unwrap();
        assert_eq!(&bytes[..4], b"MLYR");
        assert_eq!(bytes[6] & FLAG_ENCRYPTED, 0);
        let back = decode(&bytes, None).unwrap();
        assert_eq!(back.project, "demo");
    }

    #[test]
    fn round_trip_seeded() {
        let seed = "correct horse battery staple extra";
        let bytes = encode(&sample(), &params(Some(seed))).unwrap();
        assert_ne!(bytes[6] & FLAG_ENCRYPTED, 0);
        let back = decode(&bytes, Some(seed)).unwrap();
        assert_eq!(back.project, "demo");
        assert!(matches!(decode(&bytes, None), Err(SyncError::SeedRequired)));
        assert!(format!("{}", decode(&bytes, None).unwrap_err())
            .contains("cannot be imported without the seed phrase"));
        assert!(matches!(
            decode(&bytes, Some("wrong seed phrase!!")),
            Err(SyncError::InvalidSeed)
        ));
    }

    #[test]
    fn encrypted_without_seed_is_seed_required_not_corrupt() {
        let bytes = encode(
            &sample(),
            &params(Some("correct horse battery staple extra")),
        )
        .unwrap();
        let err = decode(&bytes, None).unwrap_err();
        let s = err.to_string();
        assert!(matches!(err, SyncError::SeedRequired));
        assert!(!s.to_lowercase().contains("aead"));
        assert!(!s.to_lowercase().contains("checksum"));
    }

    #[test]
    fn not_legible() {
        let bytes = encode(&sample(), &params(None)).unwrap();
        let as_str = String::from_utf8_lossy(&bytes);
        assert!(!as_str.contains("memlayer.archive"));
        assert!(!as_str.contains("demo"));
    }

    #[test]
    fn smaller_than_json_zstd3() {
        let mut p = sample();
        p.observations = (0..50)
            .map(|i| ArchivedObservation {
                id: i,
                sync_id: format!("sync-{i}"),
                session_id: "s1".into(),
                r#type: "note".into(),
                title: format!("obs-{i}-repeated-text-for-compression"),
                content: "the same long observation body repeats on every row to reward a dictionary compressor ".repeat(8),
                tool_name: None,
                scope: "project".into(),
                created_by: None,
                topic_key: Some("topic".into()),
                normalized_hash: None,
                revision_count: 1,
                duplicate_count: 1,
                last_seen_at: None,
                created_at: "2026-09-12T00:00:00Z".into(),
                updated_at: "2026-09-12T00:00:00Z".into(),
                deleted_at: None,
                review_after: None,
                code_anchor: None,
            })
            .collect();
        let mem = encode(&p, &params(None)).unwrap();
        let json = serde_json::to_vec_pretty(&p).unwrap();
        let z3 = zstd::encode_all(&json[..], 3).unwrap();
        assert!(
            mem.len() < z3.len(),
            "mem {} vs json.zst-3 {}",
            mem.len(),
            z3.len()
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut b = encode(&sample(), &params(None)).unwrap();
        b[0] = b'X';
        assert!(matches!(decode(&b, None), Err(SyncError::NotArchive)));
    }

    #[test]
    fn rejects_truncated() {
        let b = encode(&sample(), &params(None)).unwrap();
        assert!(decode(&b[..20], None).is_err());
    }

    #[test]
    fn seed_too_short() {
        assert!(encode(&sample(), &params(Some("short"))).is_err());
    }
}
