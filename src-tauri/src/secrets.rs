use crate::error::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};

use crate::providers::ProviderCredentials;

pub const SECRET_SERVICE: &str = "com.bynhat.murmur";
pub const ALLOWED_SECRET_NAMES: &[&str] = &["deepgram", "assemblyai", "groq", "github_updates"];

pub trait SecretStore: Send + Sync {
    fn set(&self, name: &str, secret: &str) -> CoreResult<()>;
    fn get(&self, name: &str) -> CoreResult<Option<String>>;
    fn delete(&self, name: &str) -> CoreResult<()>;
}

pub struct WindowsCredentialStore;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub name: String,
    pub configured: bool,
}

pub fn secret_statuses(store: &dyn SecretStore) -> CoreResult<Vec<SecretStatus>> {
    ALLOWED_SECRET_NAMES
        .iter()
        .map(|name| {
            Ok(SecretStatus {
                name: (*name).into(),
                configured: store.get(name)?.is_some(),
            })
        })
        .collect()
}

pub fn provider_credentials(store: &dyn SecretStore) -> CoreResult<ProviderCredentials> {
    Ok(ProviderCredentials {
        deepgram: store.get("deepgram")?,
        assemblyai: store.get("assemblyai")?,
        groq: store.get("groq")?,
    })
}

fn validate_name(name: &str) -> CoreResult<()> {
    if ALLOWED_SECRET_NAMES.contains(&name) {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(format!(
            "unknown secret name: {name}"
        )))
    }
}

#[cfg(windows)]
impl SecretStore for WindowsCredentialStore {
    fn set(&self, name: &str, secret: &str) -> CoreResult<()> {
        validate_name(name)?;
        if secret.trim().is_empty() {
            return Err(CoreError::InvalidInput("secret cannot be empty".into()));
        }
        keyring::Entry::new(SECRET_SERVICE, name)
            .map_err(map_keyring)?
            .set_password(secret)
            .map_err(map_keyring)
    }

    fn get(&self, name: &str) -> CoreResult<Option<String>> {
        validate_name(name)?;
        match keyring::Entry::new(SECRET_SERVICE, name)
            .map_err(map_keyring)?
            .get_password()
        {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_keyring(error)),
        }
    }

    fn delete(&self, name: &str) -> CoreResult<()> {
        validate_name(name)?;
        match keyring::Entry::new(SECRET_SERVICE, name)
            .map_err(map_keyring)?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring(error)),
        }
    }
}

#[cfg(windows)]
fn map_keyring(error: keyring::Error) -> CoreError {
    CoreError::SecretStore(error.to_string())
}

#[cfg(not(windows))]
impl SecretStore for WindowsCredentialStore {
    fn set(&self, name: &str, _secret: &str) -> CoreResult<()> {
        validate_name(name)?;
        Err(CoreError::Unavailable(
            "Windows Credential Manager is only available on Windows".into(),
        ))
    }
    fn get(&self, name: &str) -> CoreResult<Option<String>> {
        validate_name(name)?;
        Err(CoreError::Unavailable(
            "Windows Credential Manager is only available on Windows".into(),
        ))
    }
    fn delete(&self, name: &str) -> CoreResult<()> {
        validate_name(name)?;
        Err(CoreError::Unavailable(
            "Windows Credential Manager is only available on Windows".into(),
        ))
    }
}
