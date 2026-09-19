use crate::error::Error;
use crate::message::ContentBlock;
use crate::message::Message;
use crate::message::Role;
use crate::message::ToolUse;
use crate::model::Completion;
use crate::model::CompletionRequest;
use crate::model::Model;
use crate::model::StopReason;
use crate::model::Usage;
use crate::tool::ToolSpec;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::env;

const PROVIDER_NAME: &str = "anthropic";
const API_KEY_ENV_VAR: &str = "ANTHROPIC_API_KEY";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 16_000;

#[derive(Debug, Clone)]
pub struct Anthropic {
    model: String,
    api_key: Option<String>,
    base_url: String,
    max_tokens: u32,
    client: Client,
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    system: &'a str,
    messages: Vec<MessageParam<'a>>,
    #[serde(skip_serializing_if = "<[ToolSpec]>::is_empty")]
    tools: &'a [ToolSpec],
}

#[derive(Serialize)]
struct MessageParam<'a> {
    role: &'static str,
    content: Vec<ContentBlockParam<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlockParam<'a> {
    Text {
        text: &'a str,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: &'a Value,
    },
    ToolResult {
        tool_use_id: &'a str,
        content: &'a str,
        is_error: bool,
    },
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlockResponse>,
    stop_reason: Option<String>,
    usage: UsageResponse,
}

#[derive(Deserialize)]
struct UsageResponse {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlockResponse {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
}

impl Anthropic {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: env::var(API_KEY_ENV_VAR).ok(),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            client: Client::new(),
        }
    }

    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

#[async_trait]
impl Model for Anthropic {
    fn name(&self) -> &str {
        &self.model
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, Error> {
        let api_key = self.api_key.as_deref().ok_or(Error::MissingApiKey {
            provider: PROVIDER_NAME,
            env_var: API_KEY_ENV_VAR,
        })?;
        let body = MessagesRequest::from_completion_request(&self.model, self.max_tokens, &request);
        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let is_success = status.is_success();
        match is_success {
            true => parse_completion(response.json().await?),
            false => Err(Error::Provider {
                provider: PROVIDER_NAME,
                status: status.as_u16(),
                body: response.text().await?,
            }),
        }
    }
}

impl<'a> MessagesRequest<'a> {
    fn from_completion_request(
        model: &'a str,
        max_tokens: u32,
        request: &CompletionRequest<'a>,
    ) -> Self {
        Self {
            model,
            max_tokens,
            system: request.instructions,
            messages: request.messages.iter().map(MessageParam::from).collect(),
            tools: request.tools,
        }
    }
}

impl<'a> From<&'a Message> for MessageParam<'a> {
    fn from(message: &'a Message) -> Self {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        Self {
            role,
            content: message
                .content
                .iter()
                .map(ContentBlockParam::from)
                .collect(),
        }
    }
}

impl<'a> From<&'a ContentBlock> for ContentBlockParam<'a> {
    fn from(block: &'a ContentBlock) -> Self {
        match block {
            ContentBlock::Text { text } => Self::Text { text },
            ContentBlock::ToolUse(tool_use) => Self::ToolUse {
                id: &tool_use.id,
                name: &tool_use.name,
                input: &tool_use.input,
            },
            ContentBlock::ToolResult(tool_result) => Self::ToolResult {
                tool_use_id: &tool_result.tool_use_id,
                content: &tool_result.content,
                is_error: tool_result.is_error,
            },
        }
    }
}

fn parse_completion(response: MessagesResponse) -> Result<Completion, Error> {
    let usage = Usage {
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
    };
    let stop_reason = parse_stop_reason(response.stop_reason.as_deref());
    let content = response
        .content
        .into_iter()
        .filter_map(parse_content_block)
        .collect::<Vec<ContentBlock>>();
    match content.is_empty() {
        true => Err(Error::EmptyResponse),
        false => Ok(Completion {
            content,
            stop_reason,
            usage,
        }),
    }
}

fn parse_content_block(block: ContentBlockResponse) -> Option<ContentBlock> {
    match block {
        ContentBlockResponse::Text { text } => Some(ContentBlock::Text { text }),
        ContentBlockResponse::ToolUse { id, name, input } => {
            Some(ContentBlock::ToolUse(ToolUse { id, name, input }))
        }
        ContentBlockResponse::Other => None,
    }
}

fn parse_stop_reason(stop_reason: Option<&str>) -> StopReason {
    match stop_reason {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some(_) | None => StopReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolResult;
    use serde_json::json;

    #[test]
    fn serializes_tool_blocks_and_tools() {
        let messages = [
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUse {
                    id: "toolu_1".to_string(),
                    name: "echo".to_string(),
                    input: json!({ "text": "hi" }),
                })],
            ),
            Message::tool_results(vec![ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: "hi".to_string(),
                is_error: false,
            }]),
        ];
        let tools = [ToolSpec {
            name: "echo".to_string(),
            description: "Echo text.".to_string(),
            input_schema: json!({ "type": "object" }),
        }];
        let request = CompletionRequest {
            instructions: "",
            messages: &messages,
            tools: &tools,
        };
        let body = MessagesRequest::from_completion_request("model", 1, &request);
        assert_eq!(
            json!(body),
            json!({
                "model": "model",
                "max_tokens": 1,
                "messages": [
                    {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "echo",
                            "input": { "text": "hi" }
                        }]
                    },
                    {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "toolu_1",
                            "content": "hi",
                            "is_error": false
                        }]
                    }
                ],
                "tools": [{
                    "name": "echo",
                    "description": "Echo text.",
                    "input_schema": { "type": "object" }
                }]
            })
        );
    }

    #[test]
    fn omits_tools_when_empty() {
        let request = CompletionRequest {
            instructions: "",
            messages: &[],
            tools: &[],
        };
        let body = MessagesRequest::from_completion_request("model", 1, &request);
        assert!(json!(body).get("tools").is_none());
    }

    #[test]
    fn parses_tool_use_completion() {
        let response: MessagesResponse = serde_json::from_value(json!({
            "content": [
                { "type": "text", "text": "checking" },
                { "type": "thinking", "thinking": "..." },
                { "type": "tool_use", "id": "toolu_1", "name": "echo", "input": { "text": "hi" } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 3, "output_tokens": 5 }
        }))
        .unwrap();
        let completion = parse_completion(response).unwrap();
        assert_eq!(completion.stop_reason, StopReason::ToolUse);
        assert_eq!(completion.text(), "checking");
        assert_eq!(
            completion.content[1],
            ContentBlock::ToolUse(ToolUse {
                id: "toolu_1".to_string(),
                name: "echo".to_string(),
                input: json!({ "text": "hi" }),
            })
        );
    }
}
