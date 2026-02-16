//! Keychain storage for authentication tokens.
//!
//! This module provides secure storage for refresh tokens using pluggable backends.
//! Available backends are controlled via Cargo features:
//! - `keychain-macos`: macOS Keychain (default on macOS)
//! - `keychain-file`: TOML file in config directory

use std::io;

#[cfg(target_os = "macos")]
pub use macos::platform_keychain;

#[cfg(target_os = "linux")]
pub use linux::platform_keychain;

/// Account name for storing the OAuth refresh token
const REFRESH_TOKEN_KEY: &str = "refresh-token";

/// A backend for storing and retrieving secrets.
pub trait SecretBackend {
    fn store_refresh_token(&self, token: &str) -> io::Result<()> {
        self.write_secret(REFRESH_TOKEN_KEY, token)
    }

    fn get_refresh_token(&self) -> io::Result<Option<String>> {
        self.read_secret(REFRESH_TOKEN_KEY)
    }

    #[allow(dead_code)]
    fn delete_refresh_token(&self) -> io::Result<()> {
        self.delete_secret(REFRESH_TOKEN_KEY)
    }

    /// Read a secret by name.
    /// Returns `Ok(None)` if the secret does not exist.
    fn read_secret(&self, name: &str) -> io::Result<Option<String>>;

    /// Write a secret by name.
    fn write_secret(&self, name: &str, value: &str) -> io::Result<()>;

    /// Delete a secret by name.
    fn delete_secret(&self, name: &str) -> io::Result<()>;
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::LazyLock;

    use super::*;
    use libsecret::*;

    pub fn platform_keychain() -> Box<dyn SecretBackend> {
        // This conditional compilation is little bit cryptic, but it does
        // make sure that whatever profile we building (release/debug) we don't
        // get dead code warning, without any explicit `#[allow(dead_code)]`
        #[cfg(debug_assertions)]
        if cfg!(debug_assertions) {
            Box::new(file::FileBackend::new())
        } else {
            Box::new(LibsecretBackend)
        }

        #[cfg(not(debug_assertions))]
        Box::new(LibsecretBackend)
    }

    const NAME_ATTRIBUTE_KEY: &str = "name";

    /// Wrapper providing Sync and Send impl for libsecret::Schema to allow the use with LazyLock.
    struct SchemaHandle {
        schema: Schema,
    }

    unsafe impl Send for SchemaHandle {}
    unsafe impl Sync for SchemaHandle {}

    static SCHEMA: LazyLock<SchemaHandle> = LazyLock::new(|| {
        let mut attributes = HashMap::new();
        attributes.insert(NAME_ATTRIBUTE_KEY, SchemaAttributeType::String);

        let schema = Schema::new("com.jetbrains.acp-jcp", SchemaFlags::NONE, attributes);

        SchemaHandle { schema }
    });

    struct LibsecretBackend;

    impl SecretBackend for LibsecretBackend {
        fn read_secret(&self, name: &str) -> io::Result<Option<String>> {
            let mut attributes = HashMap::new();
            attributes.insert(NAME_ATTRIBUTE_KEY, name);
            password_lookup_sync(Some(&SCHEMA.schema), attributes, gio::Cancellable::NONE)
                .map_err(io::Error::other)
                .map(|maybe_gstring| maybe_gstring.map(|gstring| gstring.to_string()))
        }

        fn write_secret(&self, name: &str, value: &str) -> io::Result<()> {
            let mut attributes = HashMap::new();
            attributes.insert(NAME_ATTRIBUTE_KEY, name);
            password_store_sync(
                Some(&SCHEMA.schema),
                attributes,
                Some(&COLLECTION_DEFAULT),
                "JetBrains ACP JCP Password",
                value,
                gio::Cancellable::NONE,
            )
            .map_err(io::Error::other)
        }

        fn delete_secret(&self, name: &str) -> io::Result<()> {
            let mut attributes = HashMap::new();
            attributes.insert(NAME_ATTRIBUTE_KEY, name);
            password_clear_sync(Some(&SCHEMA.schema), attributes, gio::Cancellable::NONE)
                .map_err(io::Error::other)
        }
    }
}

#[cfg(target_os = "macos")]
/// macOS Keychain backend for secret storage.
mod macos {
    use super::*;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    pub fn platform_keychain() -> Box<dyn SecretBackend> {
        // This conditional compilation is little bit cryptic, but it does
        // make sure that whatever profile we building (release/debug) we don't
        // get dead code warning, without any explicit `#[allow(dead_code)]`
        #[cfg(debug_assertions)]
        if cfg!(debug_assertions) {
            Box::new(file::FileBackend::new())
        } else {
            Box::new(MacOsBackend)
        }

        #[cfg(not(debug_assertions))]
        Box::new(MacOsBackend)
    }

    /// Service identifier for Keychain storage
    const SERVICE: &str = "com.jetbrains.acp-jcp";

    const OS_STATUS_NOT_FOUND: i32 = -25300;

    /// macOS Keychain backend.
    pub struct MacOsBackend;

    impl SecretBackend for MacOsBackend {
        fn read_secret(&self, name: &str) -> io::Result<Option<String>> {
            match get_generic_password(SERVICE, name) {
                Ok(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|e| io::Error::other(format!("Invalid UTF-8: {}", e))),
                Err(e) if e.code() == OS_STATUS_NOT_FOUND => Ok(None),
                Err(e) => Err(io::Error::other(e)),
            }
        }

        fn write_secret(&self, name: &str, value: &str) -> io::Result<()> {
            set_generic_password(SERVICE, name, value.as_bytes()).map_err(io::Error::other)
        }

        fn delete_secret(&self, name: &str) -> io::Result<()> {
            match delete_generic_password(SERVICE, name) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == OS_STATUS_NOT_FOUND => Ok(()),
                Err(e) => Err(io::Error::other(e)),
            }
        }
    }
}

/// TOML file backend for secret storage.
///
/// Stores secrets in a TOML file located in the user's config directory. Only used in local debug build,
/// because macOS asks keychain password each time executable is recompiled no matter what settings are
/// configured for a keychain item and how the binary is signed.
///
/// If you'll find a way to disable this behaviour on macOS, please remove [`FileBackend`]. There is no
/// other reasosns to use it.
///
/// This module is intentonally marked as `cfg(debug_assertions)` in order to guarantee that it will
/// never be published and unintentionally used to store secrets in the production context.
#[cfg(debug_assertions)]
mod file {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::{collections::HashMap, fs, io, path::PathBuf};

    const CONFIG_FILE_NAME: &str = "secrets.toml";
    const APP_NAME: &str = "jcp";

    /// File-based backend that stores secrets in a TOML file.
    pub struct FileBackend {
        path: PathBuf,
    }

    #[derive(Serialize, Deserialize, Default)]
    struct SecretsFile {
        #[serde(default)]
        secrets: HashMap<String, String>,
    }

    impl FileBackend {
        pub fn new() -> Self {
            let path = dirs::config_dir()
                .expect("Unable to red config dir")
                .join(APP_NAME)
                .join(CONFIG_FILE_NAME);
            Self { path }
        }

        fn read_file(&self) -> io::Result<SecretsFile> {
            if !self.path.exists() {
                return Ok(SecretsFile::default());
            }
            let content = fs::read_to_string(&self.path)?;
            toml::from_str(&content).map_err(io::Error::other)
        }

        fn write_file(&self, secrets: &SecretsFile) -> io::Result<()> {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = toml::to_string_pretty(secrets).map_err(io::Error::other)?;
            fs::write(&self.path, content)
        }

        #[cfg(test)]
        fn with_path(path: PathBuf) -> Self {
            Self { path }
        }
    }

    impl SecretBackend for FileBackend {
        fn read_secret(&self, name: &str) -> io::Result<Option<String>> {
            let secrets = self.read_file()?;
            Ok(secrets.secrets.get(name).cloned())
        }

        fn write_secret(&self, name: &str, value: &str) -> io::Result<()> {
            let mut secrets = self.read_file()?;
            secrets.secrets.insert(name.to_string(), value.to_string());
            self.write_file(&secrets)
        }

        fn delete_secret(&self, name: &str) -> io::Result<()> {
            let mut secrets = self.read_file()?;
            secrets.secrets.remove(name);
            self.write_file(&secrets)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn test_file_backend_read_write() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("secrets.toml");
            let backend = FileBackend::with_path(path.clone());

            // Initially empty
            assert_eq!(backend.read_secret("test-key").unwrap(), None);

            // Write and read back
            backend.write_secret("test-key", "test-value").unwrap();
            assert_eq!(
                backend.read_secret("test-key").unwrap(),
                Some("test-value".to_string())
            );

            // Verify file content
            let content = fs::read_to_string(&path).unwrap();
            assert!(content.contains("test-key"));
            assert!(content.contains("test-value"));
        }

        #[test]
        fn test_file_backend_delete() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("secrets.toml");
            let backend = FileBackend::with_path(path);

            backend.write_secret("key1", "value1").unwrap();
            backend.write_secret("key2", "value2").unwrap();

            backend.delete_secret("key1").unwrap();

            assert_eq!(backend.read_secret("key1").unwrap(), None);
            assert_eq!(
                backend.read_secret("key2").unwrap(),
                Some("value2".to_string())
            );
        }
    }
}
