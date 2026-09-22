#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficScope {
    Team { team_id: u64, user_id: u64 },
    Personal { user_id: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLimitDefaults {
    pub team_mbps: u32,
    pub member_mbps: u32,
    pub personal_mbps: u32,
}

impl RelayLimitDefaults {
    pub fn validate(self) -> Result<Self, LimitConfigError> {
        if self.team_mbps == 0 || self.member_mbps == 0 || self.personal_mbps == 0 {
            return Err(LimitConfigError::ZeroRate);
        }
        Ok(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LimitConfigError {
    #[error("relay rate limits must be greater than zero")]
    ZeroRate,
}

pub fn mbps_to_bytes_per_second(mbps: u32) -> u64 {
    u64::from(mbps) * 1_000_000 / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_decimal_megabits_to_bytes() {
        assert_eq!(mbps_to_bytes_per_second(20), 2_500_000);
    }

    #[test]
    fn rejects_zero_as_an_implicit_unlimited_value() {
        let result = RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 0,
            personal_mbps: 5,
        }
        .validate();
        assert_eq!(result, Err(LimitConfigError::ZeroRate));
    }
}
