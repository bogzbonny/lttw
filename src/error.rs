/// `nvim-oxi`'s result type.
pub type LttwResult<T> = std::result::Result<T, Error>;

/// Error type for FIM operations
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Lttw plugin error: {0}")]
    Lttw(String),

    #[error("Server error: {0}")]
    Server(String),

    #[error("nvim_oxi error: {0}")]
    NvimOxi(#[from] nvim_oxi::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("tokio JoinError error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
}

impl From<nvim_oxi::api::Error> for Error {
    fn from(err: nvim_oxi::api::Error) -> Self {
        Error::NvimOxi(err.into())
    }
}
