use crate::error::Error;
use crate::message::Message;
use crate::message::Role;
use crate::model::Completion;
use crate::model::CompletionRequest;
use crate::model::Model;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
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
}

#[derive(Serialize)]
struct MessageParam<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
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
            content: &message.content,
        }
    }
}

fn parse_completion(response: MessagesResponse) -> Result<Completion, Error> {
    let text = response
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text),
            ContentBlock::Other => None,
        })
        .collect::<Vec<String>>()
        .join("");
    match text.is_empty() {
        true => Err(Error::EmptyResponse),
        false => Ok(Completion { text }),
    }
}
