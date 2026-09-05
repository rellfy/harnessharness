use thiserror::Error;

const RETRYABLE_STATUSES: [u16; 3] = [408, 409, 429];

#[derive(Debug, Error)]
pub enum Error {
    #[error("harness requires a model; call `.model(...)` on the builder")]
    MissingModel,
    #[error("missing API key for provider `{provider}`; set `{env_var}`")]
    MissingApiKey {
        provider: &'static str,
        env_var: &'static str,
    },
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider `{provider}` returned status {status}: {body}")]
    Provider {
        provider: &'static str,
        status: u16,
        body: String,
    },
    #[error("provider returned no text content")]
    EmptyResponse,
}

impl Error {
    pub fn get_is_retryable(&self) -> bool {
        match self {
            Error::Http(error) => error.is_timeout() || error.is_connect() || error.is_body(),
            Error::Provider { status, .. } => *status >= 500 || RETRYABLE_STATUSES.contains(status),
            Error::MissingModel | Error::MissingApiKey { .. } | Error::EmptyResponse => false,
        }
    }
}
