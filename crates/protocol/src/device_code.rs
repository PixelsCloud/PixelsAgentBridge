use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A deployment-scoped, human-readable device locator. It is not a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceCode(u32);

impl DeviceCode {
    pub fn new(value: u32) -> Result<Self, DeviceCodeError> {
        (100_000_000..=999_999_999)
            .contains(&value)
            .then_some(Self(value))
            .ok_or(DeviceCodeError)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for DeviceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DeviceCode {
    type Err = DeviceCodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 9 || !value.as_bytes().iter().all(u8::is_ascii_digit) {
            return Err(DeviceCodeError);
        }
        Self::new(value.parse().map_err(|_| DeviceCodeError)?)
    }
}

impl Serialize for DeviceCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeviceCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("device code must be exactly nine ASCII digits, starting with 1–9")]
pub struct DeviceCodeError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_exactly_nine_ascii_digits() {
        assert!("123456789".parse::<DeviceCode>().is_ok());
        for invalid in [
            "012345678",
            "12345678",
            "1234567890",
            "123 456 789",
            "１２３４５６７８９",
        ] {
            assert!(invalid.parse::<DeviceCode>().is_err());
        }
    }

    #[test]
    fn wire_format_is_a_string() {
        let code = DeviceCode::new(123_456_789).unwrap();
        assert_eq!(serde_json::to_string(&code).unwrap(), "\"123456789\"");
        assert_eq!(
            serde_json::from_str::<DeviceCode>("\"123456789\"").unwrap(),
            code
        );
        assert!(serde_json::from_str::<DeviceCode>("123456789").is_err());
    }
}
