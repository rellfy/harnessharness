pub mod provider;

use crate::error::Error;
use crate::message::Message;
use async_trait::async_trait;

#[async_trait]
pub trait Model: Send + Sync {
    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, Error>;
}

#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    pub instructions: &'a str,
    pub messages: &'a [Message],
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
}
