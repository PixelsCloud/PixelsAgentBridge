use argon2::{
    Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _, password_hash::SaltString,
};
use rand_core::OsRng;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone)]
pub struct PasswordPolicy {
    pub min_characters: usize,
    pub max_bytes: usize,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_characters: 10,
            max_bytes: 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PasswordEngine {
    policy: PasswordPolicy,
}

impl PasswordEngine {
    pub fn new(policy: PasswordPolicy) -> Self {
        Self { policy }
    }

    pub fn hash(&self, password: &str) -> Result<String, CredentialError> {
        self.validate(password)?;
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| CredentialError::Hash(error.to_string()))
    }

    pub fn verify(&self, password: &str, encoded_hash: &str) -> bool {
        let Ok(hash) = PasswordHash::new(encoded_hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    }

    fn validate(&self, password: &str) -> Result<(), CredentialError> {
        if password.len() > self.policy.max_bytes {
            return Err(CredentialError::PasswordTooLong);
        }
        if password.chars().count() < self.policy.min_characters {
            return Err(CredentialError::PasswordTooShort);
        }
        Ok(())
    }
}

pub fn normalize_username(input: &str) -> Result<(String, String), CredentialError> {
    let display = input.trim().nfkc().collect::<String>();
    let count = display.chars().count();
    if !(3..=64).contains(&count)
        || display.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || character == '/'
                || character == '\\'
        })
    {
        return Err(CredentialError::InvalidUsername);
    }
    let key = display.to_lowercase();
    Ok((display, key))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("username must contain 3 to 64 non-whitespace characters")]
    InvalidUsername,
    #[error("password is shorter than the configured minimum")]
    PasswordTooShort,
    #[error("password exceeds the configured byte limit")]
    PasswordTooLong,
    #[error("password hashing failed: {0}")]
    Hash(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_compatibility_characters_and_case() {
        let (display, key) = normalize_username("  Ａlice  ").unwrap();
        assert_eq!(display, "Alice");
        assert_eq!(key, "alice");
    }

    #[test]
    fn hashes_and_verifies_without_storing_plaintext() {
        let engine = PasswordEngine::new(PasswordPolicy::default());
        let encoded = engine.hash("correct horse battery staple").unwrap();
        assert!(!encoded.contains("correct horse"));
        assert!(engine.verify("correct horse battery staple", &encoded));
        assert!(!engine.verify("wrong password", &encoded));
    }
}
