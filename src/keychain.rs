//! Keychain storage for authentication tokens (macOS only).
//!
//! This module provides secure storage for refresh tokens using the macOS Keychain.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use std::string::FromUtf8Error;
use thiserror::Error;

/// Service identifier for Keychain storage
const SERVICE: &str = "com.jetbrains.acp-jcp";

/// Account name for storing the OAuth refresh token
const REFRESH_TOKEN_ACCOUNT: &str = "refresh-token";

const OS_STATUS_NOT_FOUND: i32 = -25300;

#[derive(Error, Debug)]
pub enum KeychainError {
    #[error("Keychain operation failed: {0}")]
    SecurityFramework(#[from] security_framework::base::Error),

    #[error("Invalid UTF-8 in stored token")]
    InvalidUtf8(#[from] FromUtf8Error),
}

pub fn store_refresh_token(token: &str) -> Result<(), KeychainError> {
    set_generic_password(SERVICE, REFRESH_TOKEN_ACCOUNT, token.as_bytes())?;
    Ok(())
}

pub fn get_refresh_token() -> Result<Option<String>, KeychainError> {
    match get_generic_password(SERVICE, REFRESH_TOKEN_ACCOUNT) {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
        Err(e) if e.code() == OS_STATUS_NOT_FOUND => Ok(None),
        Err(e) => Err(KeychainError::SecurityFramework(e)),
    }
}

#[allow(dead_code)]
pub fn delete_refresh_token() -> Result<(), KeychainError> {
    delete_generic_password(SERVICE, REFRESH_TOKEN_ACCOUNT)?;
    Ok(())
}
