use thiserror::Error;

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
