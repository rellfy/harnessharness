pub mod provider;

use crate::error::Error;
use crate::message::ContentBlock;
use crate::message::Message;
use crate::message::join_text;
use crate::tool::ToolSpec;
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
    pub tools: &'a [ToolSpec],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Completion {
    pub fn text(&self) -> String {
        join_text(&self.content)
    }
}
