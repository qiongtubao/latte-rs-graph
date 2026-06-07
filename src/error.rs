use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GraphError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Tree-sitter error: {0}")]
    TreeSitter(String),

    #[error("Parse error in {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("Language not supported: {0}")]
    UnsupportedLanguage(String),

    #[error("Engine error: {0}")]
    Engine(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid configuration: {0}")]
    Config(String),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type GraphResult<T> = Result<T, GraphError>;
