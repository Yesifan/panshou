use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;

use crate::config::AppPaths;

use super::{StateCipher, StateKeyError};

const ENVELOPE_MARKER: &str = "__pansou_secret_v1";

#[derive(Clone)]
pub struct StateStore {
    root: PathBuf,
    cipher: Option<StateCipher>,
}

impl std::fmt::Debug for StateStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StateStore")
            .field("root", &self.root)
            .field("encrypted", &self.cipher.is_some())
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("invalid provider or profile name {0:?}")]
    InvalidName(String),
    #[error("failed to access provider state {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid provider state {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
    #[error(transparent)]
    Key(#[from] StateKeyError),
    #[error("password persistence requires PANSOU_STATE_KEY")]
    PasswordRequiresKey,
    #[error("encrypted provider state requires PANSOU_STATE_KEY")]
    MissingKey,
}

impl StateStore {
    /// Construct a store and read `PANSOU_STATE_KEY`, if present.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StateError> {
        let cipher = env::var("PANSOU_STATE_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| StateCipher::parse(&value))
            .transpose()?;
        Ok(Self {
            root: root.into(),
            cipher,
        })
    }

    pub fn from_paths(paths: &AppPaths) -> Result<Self, StateError> {
        Self::new(&paths.providers_dir)
    }

    pub fn with_cipher(root: impl Into<PathBuf>, cipher: Option<StateCipher>) -> Self {
        Self {
            root: root.into(),
            cipher,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn encryption_enabled(&self) -> bool {
        self.cipher.is_some()
    }

    pub fn profile_path(&self, provider: &str, profile: &str) -> Result<PathBuf, StateError> {
        validate_name(provider)?;
        validate_name(profile)?;
        Ok(self.root.join(provider).join(format!("{profile}.json")))
    }

    pub fn save<T: Serialize>(
        &self,
        provider: &str,
        profile: &str,
        state: &T,
    ) -> Result<(), StateError> {
        let path = self.profile_path(provider, profile)?;
        fs::create_dir_all(&self.root).map_err(|source| io_error(&self.root, source))?;
        set_dir_private(&self.root)?;
        let mut value = serde_json::to_value(state).map_err(|source| StateError::Json {
            path: path.display().to_string(),
            source,
        })?;
        protect_secrets(&mut value, None, self.cipher.as_ref())?;
        let bytes = serde_json::to_vec_pretty(&value).map_err(|source| StateError::Json {
            path: path.display().to_string(),
            source,
        })?;
        atomic_private_write(&path, &bytes)
    }

    pub fn load<T: DeserializeOwned>(
        &self,
        provider: &str,
        profile: &str,
    ) -> Result<Option<T>, StateError> {
        let path = self.profile_path(provider, profile)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io_error(&path, source)),
        };
        let mut value: Value =
            serde_json::from_slice(&bytes).map_err(|source| StateError::Json {
                path: path.display().to_string(),
                source,
            })?;
        reveal_secrets(&mut value, None, self.cipher.as_ref())?;
        serde_json::from_value(value)
            .map(Some)
            .map_err(|source| StateError::Json {
                path: path.display().to_string(),
                source,
            })
    }

    pub fn delete(&self, provider: &str, profile: &str) -> Result<bool, StateError> {
        let path = self.profile_path(provider, profile)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(io_error(&path, source)),
        }
    }

    pub fn list_profiles(&self, provider: &str) -> Result<Vec<String>, StateError> {
        validate_name(provider)?;
        let dir = self.root.join(provider);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(io_error(&dir, source)),
        };
        let mut profiles = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| io_error(&dir, source))?;
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) == Some("json")
                && let Some(name) = path.file_stem().and_then(|v| v.to_str())
            {
                profiles.push(name.to_owned());
            }
        }
        profiles.sort_unstable();
        Ok(profiles)
    }
}

fn protect_secrets(
    value: &mut Value,
    field: Option<&str>,
    cipher: Option<&StateCipher>,
) -> Result<(), StateError> {
    if field.is_some_and(is_password_field) && cipher.is_none() && !empty_secret(value) {
        return Err(StateError::PasswordRequiresKey);
    }
    if let (Some(field), Some(cipher)) = (field.filter(|name| is_secret_field(name)), cipher)
        && !value.is_null()
        && !empty_secret(value)
    {
        let plaintext = serde_json::to_vec(value).expect("serializing a JSON value cannot fail");
        let (nonce, ciphertext) = cipher.encrypt(&plaintext, field.as_bytes())?;
        *value = json!({ ENVELOPE_MARKER: true, "alg": "AES-256-GCM", "nonce": nonce, "ciphertext": ciphertext });
        return Ok(());
    }
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                protect_secrets(child, Some(name), cipher)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                protect_secrets(child, field, cipher)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reveal_secrets(
    value: &mut Value,
    field: Option<&str>,
    cipher: Option<&StateCipher>,
) -> Result<(), StateError> {
    if let Value::Object(map) = value {
        if map.get(ENVELOPE_MARKER) == Some(&Value::Bool(true)) {
            let cipher = cipher.ok_or(StateError::MissingKey)?;
            let nonce = map
                .get("nonce")
                .and_then(Value::as_str)
                .ok_or(StateKeyError::MalformedCiphertext)?;
            let ciphertext = map
                .get("ciphertext")
                .and_then(Value::as_str)
                .ok_or(StateKeyError::MalformedCiphertext)?;
            let plaintext = cipher.decrypt(nonce, ciphertext, field.unwrap_or("").as_bytes())?;
            *value = serde_json::from_slice(&plaintext)
                .map_err(|_| StateKeyError::MalformedCiphertext)?;
            return Ok(());
        }
        for (name, child) in map {
            reveal_secrets(child, Some(name), cipher)?;
        }
    } else if let Value::Array(values) = value {
        for child in values {
            reveal_secrets(child, field, cipher)?;
        }
    }
    Ok(())
}

fn is_secret_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('-', "_");
    let parts = name.split('_').collect::<Vec<_>>();
    is_password_field(&name)
        || parts
            .iter()
            .any(|part| matches!(*part, "cookie" | "cookies" | "token"))
        || matches!(name.as_str(), "authorization" | "app_auth" | "phpsessid")
}

fn is_password_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('-', "_");
    name.split('_').any(|part| part == "password")
}

fn empty_secret(value: &Value) -> bool {
    value.is_null() || matches!(value, Value::String(v) if v.is_empty())
}

fn validate_name(value: &str) -> Result<(), StateError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'-' | b'_'))
    {
        return Err(StateError::InvalidName(value.to_owned()));
    }
    Ok(())
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let parent = path.parent().expect("profile paths have a parent");
    fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    set_dir_private(parent)?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| io_error(parent, source))?;
    set_file_private(temp.path())?;
    temp.write_all(bytes)
        .map_err(|source| io_error(temp.path(), source))?;
    temp.write_all(b"\n")
        .map_err(|source| io_error(temp.path(), source))?;
    temp.as_file()
        .sync_all()
        .map_err(|source| io_error(temp.path(), source))?;
    temp.persist(path)
        .map_err(|error| io_error(path, error.error))?;
    set_file_private(path)?;
    sync_dir(parent)
}

#[cfg(unix)]
fn set_dir_private(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| io_error(path, source))
}
#[cfg(not(unix))]
fn set_dir_private(_path: &Path) -> Result<(), StateError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_private(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|source| io_error(path, source))
}
#[cfg(not(unix))]
fn set_file_private(_path: &Path) -> Result<(), StateError> {
    Ok(())
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), StateError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| io_error(path, source))
}
#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> Result<(), StateError> {
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> StateError {
    StateError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Profile {
        username: String,
        cookie: String,
        password: Option<String>,
    }

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pansou-state-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn plaintext_cookie_is_allowed_but_password_is_not() {
        let root = test_dir("plaintext");
        let store = StateStore::with_cipher(&root, None);
        let cookie_only = Profile {
            username: "alice".into(),
            cookie: "sid=secret".into(),
            password: None,
        };
        store.save("qqpd", "main", &cookie_only).unwrap();
        assert_eq!(
            store.load::<Profile>("qqpd", "main").unwrap(),
            Some(cookie_only)
        );
        let password = Profile {
            username: "alice".into(),
            cookie: String::new(),
            password: Some("secret".into()),
        };
        assert!(matches!(
            store.save("gying", "main", &password),
            Err(StateError::PasswordRequiresKey)
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn encrypted_round_trip_does_not_store_secrets() {
        let root = test_dir("encrypted");
        let store = StateStore::with_cipher(&root, Some(StateCipher::from_bytes([9; 32])));
        let profile = Profile {
            username: "alice".into(),
            cookie: "sid=secret".into(),
            password: Some("hunter2".into()),
        };
        store.save("gying", "main", &profile).unwrap();
        let raw = fs::read_to_string(store.profile_path("gying", "main").unwrap()).unwrap();
        assert!(!raw.contains("sid=secret"));
        assert!(!raw.contains("hunter2"));
        assert_eq!(
            store.load::<Profile>("gying", "main").unwrap(),
            Some(profile)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_path_traversal() {
        let store = StateStore::with_cipher(test_dir("traversal"), None);
        assert!(matches!(
            store.profile_path("../qqpd", "main"),
            Err(StateError::InvalidName(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn writes_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let root = test_dir("mode");
        let store = StateStore::with_cipher(&root, None);
        let profile = Profile {
            username: "alice".into(),
            cookie: "sid=x".into(),
            password: None,
        };
        store.save("qqpd", "main", &profile).unwrap();
        let mode = fs::metadata(store.profile_path("qqpd", "main").unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(root);
    }
}
