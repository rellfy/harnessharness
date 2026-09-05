pub mod provider;

use crate::error::Error;
use crate::message::Message;
use async_trait::async_trait;

#[async_trait]
pub trait Model: Send + Sync {
    fn name(&self) -> &str;

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
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
