use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Keine Verbindung zum Server: {0}")]
    Network(String),
    #[error("Anmeldung erforderlich: {0}")]
    Auth(String),
    #[error("Microsoft Graph Fehler ({status}, {code}): {message}")]
    Graph {
        status: u16,
        code: String,
        message: String,
    },
    #[error("Nicht gefunden: {0}")]
    NotFound(String),
    #[error("Ungültige Eingabe: {0}")]
    Invalid(String),
    #[error("Lokaler Speicher: {0}")]
    Store(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::NotFound(_)) || matches!(self, Error::Graph { status: 404, .. })
    }

    pub fn is_network(&self) -> bool {
        matches!(self, Error::Network(_))
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Store(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Invalid(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
