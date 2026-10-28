//! Compact, canonical path identity. This is never a file-content transport.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serializer, de::Error};

fn decode<'de, D: Deserializer<'de>>(deserializer: D, limit: usize) -> Result<Vec<u8>, D::Error> {
    let encoded = String::deserialize(deserializer)?;
    if encoded.len() > limit.div_ceil(3) * 4 {
        return Err(D::Error::custom("native path exceeds wire limit"));
    }
    let decoded = STANDARD.decode(&encoded).map_err(D::Error::custom)?;
    if decoded.is_empty() || decoded.len() > limit {
        return Err(D::Error::custom("invalid native path size"));
    }
    Ok(decoded)
}

pub(crate) mod unix_bytes {
    use super::*;
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        decode(deserializer, crate::MAX_NATIVE_PATH_UNITS)
    }
}
pub(crate) mod windows_wide {
    use super::*;
    pub fn serialize<S: Serializer>(units: &[u16], serializer: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<_> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u16>, D::Error> {
        let bytes = decode(deserializer, crate::MAX_NATIVE_PATH_UNITS * 2)?;
        if bytes.len() % 2 != 0 {
            return Err(D::Error::custom("invalid UTF-16 byte count"));
        }
        Ok(bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect())
    }
}
