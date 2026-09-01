use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("secret store error: {0}")]
    SecretStore(String),
    #[error("platform feature unavailable: {0}")]
    Unavailable(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<CoreError> for CommandError {
    fn from(value: CoreError) -> Self {
        let code = match value {
            CoreError::Database(_) => "database",
            CoreError::Io(_) => "io",
            CoreError::Serialization(_) => "serialization",
            CoreError::InvalidInput(_) => "invalid_input",
            CoreError::NotFound(_) => "not_found",
            CoreError::SecretStore(_) => "secret_store",
            CoreError::Unavailable(_) => "unavailable",
        };
        Self {
            code,
            message: value.to_string(),
        }
    }
}

impl From<std::io::Error> for CommandError {
    fn from(value: std::io::Error) -> Self {
        CoreError::Io(value).into()
    }
}
