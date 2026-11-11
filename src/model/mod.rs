pub mod provider;
pub(crate) mod sse;

use crate::error::Error;
use crate::message::ContentBlock;
use crate::message::Message;
use crate::message::join_text;
use crate::tool::ToolSpec;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

#[async_trait]
pub trait Model: Send + Sync {
    fn name(&self) -> &str;

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, Error>;

    /// Complete while reporting incremental output to `on_event`.
    /// The default completes in one piece and then reports the whole content, so every model
    /// streams, if coarsely.
    async fn stream(
        &self,
        request: CompletionRequest<'_>,
        on_event: &StreamSink<'_>,
    ) -> Result<Completion, Error> {
        let completion = self.complete(request).await?;
        report_content(&completion.content, on_event);
        Ok(completion)
    }
}

pub type StreamSink<'a> = dyn Fn(StreamEvent) + Send + Sync + 'a;

/// Incremental output of a model response, in arrival order.
/// `index` is the position of the content block in the final message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// A model attempt starts; output reported by an earlier attempt of the same call is void.
    /// Sent by the agent, not by models.
    ResponseStarted,
    TextDelta {
        index: usize,
        text: String,
    },
    ToolUseStarted {
        index: usize,
        id: String,
        name: String,
    },
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

/// Token counts are exclusive: `input_tokens` leaves out tokens read from or written to the
/// prompt cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

impl Completion {
    pub fn text(&self) -> String {
        join_text(&self.content)
    }
}

fn report_content(content: &[ContentBlock], on_event: &StreamSink<'_>) {
    for (index, block) in content.iter().enumerate() {
        match block {
            ContentBlock::Text { text } => on_event(StreamEvent::TextDelta {
                index,
                text: text.clone(),
            }),
            ContentBlock::ToolUse(tool_use) => on_event(StreamEvent::ToolUseStarted {
                index,
                id: tool_use.id.clone(),
                name: tool_use.name.clone(),
            }),
            ContentBlock::ToolResult(_) | ContentBlock::Compaction(_) => {}
        }
    }
}
