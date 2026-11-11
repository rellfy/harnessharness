use super::ContentBlockResponse;
use super::PROVIDER_NAME;
use super::UsageResponse;
use super::build_completion;
use crate::error::Error;
use crate::model::Completion;
use crate::model::StreamEvent;
use crate::model::StreamSink;
use crate::model::Usage;
use crate::model::sse::SseDecoder;
use reqwest::Response;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
struct StreamAccumulator {
    blocks: BTreeMap<usize, BlockState>,
    kept_block_count: usize,
    usage: Usage,
    stop_reason: Option<String>,
    is_stopped: bool,
}

#[derive(Debug)]
enum BlockState {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input_json: String,
    },
    Ignored,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EventResponse {
    MessageStart {
        message: MessageStartResponse,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockResponse,
    },
    ContentBlockDelta {
        index: usize,
        delta: DeltaResponse,
    },
    MessageDelta {
        delta: MessageDeltaResponse,
        usage: Option<UsageDeltaResponse>,
    },
    MessageStop,
    Error {
        error: ErrorResponse,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageStartResponse {
    usage: UsageResponse,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DeltaResponse {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageDeltaResponse {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct UsageDeltaResponse {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(rename = "type")]
    kind: String,
}

pub(super) async fn read_completion(
    mut response: Response,
    on_event: &StreamSink<'_>,
) -> Result<Completion, Error> {
    let mut decoder = SseDecoder::default();
    let mut accumulator = StreamAccumulator::default();
    while !accumulator.is_stopped {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        for payload in decoder.push(&chunk) {
            accumulator.apply_payload(&payload, on_event)?;
        }
    }
    accumulator.finish()
}

impl StreamAccumulator {
    fn apply_payload(&mut self, payload: &str, on_event: &StreamSink<'_>) -> Result<(), Error> {
        let event = serde_json::from_str(payload)
            .map_err(|error| stream_error(format!("unreadable event: {error}")))?;
        self.apply(event, payload, on_event)
    }

    fn apply(
        &mut self,
        event: EventResponse,
        payload: &str,
        on_event: &StreamSink<'_>,
    ) -> Result<(), Error> {
        match event {
            EventResponse::MessageStart { message } => self.usage = Usage::from(message.usage),
            EventResponse::ContentBlockStart {
                index,
                content_block,
            } => self.start_block(index, content_block, on_event),
            EventResponse::ContentBlockDelta { index, delta } => {
                self.apply_delta(index, delta, on_event)
            }
            EventResponse::MessageDelta { delta, usage } => self.apply_message_delta(delta, usage),
            EventResponse::MessageStop => self.is_stopped = true,
            EventResponse::Error { error } => return Err(provider_error(&error, payload)),
            EventResponse::Other => {}
        }
        Ok(())
    }

    fn start_block(
        &mut self,
        index: usize,
        content_block: ContentBlockResponse,
        on_event: &StreamSink<'_>,
    ) {
        let kept_index = self.kept_block_count;
        let block = match content_block {
            ContentBlockResponse::Text { text } => {
                report_text(kept_index, &text, on_event);
                BlockState::Text { text }
            }
            ContentBlockResponse::ToolUse { id, name, .. } => {
                on_event(StreamEvent::ToolUseStarted {
                    index: kept_index,
                    id: id.clone(),
                    name: name.clone(),
                });
                BlockState::ToolUse {
                    id,
                    name,
                    input_json: String::new(),
                }
            }
            ContentBlockResponse::Other => BlockState::Ignored,
        };
        if !matches!(block, BlockState::Ignored) {
            self.kept_block_count += 1;
        }
        self.blocks.insert(index, block);
    }

    fn apply_delta(&mut self, index: usize, delta: DeltaResponse, on_event: &StreamSink<'_>) {
        let kept_index = self.count_kept_blocks_before(index);
        let Some(block) = self.blocks.get_mut(&index) else {
            return;
        };
        match (block, delta) {
            (BlockState::Text { text }, DeltaResponse::TextDelta { text: delta }) => {
                report_text(kept_index, &delta, on_event);
                text.push_str(&delta);
            }
            (
                BlockState::ToolUse { input_json, .. },
                DeltaResponse::InputJsonDelta { partial_json },
            ) => input_json.push_str(&partial_json),
            _ => {}
        }
    }

    fn apply_message_delta(
        &mut self,
        delta: MessageDeltaResponse,
        usage: Option<UsageDeltaResponse>,
    ) {
        if delta.stop_reason.is_some() {
            self.stop_reason = delta.stop_reason;
        }
        let Some(usage) = usage else {
            return;
        };
        let current = self.usage;
        self.usage = Usage {
            input_tokens: usage.input_tokens.unwrap_or(current.input_tokens),
            output_tokens: usage.output_tokens.unwrap_or(current.output_tokens),
            cache_read_input_tokens: usage
                .cache_read_input_tokens
                .unwrap_or(current.cache_read_input_tokens),
            cache_creation_input_tokens: usage
                .cache_creation_input_tokens
                .unwrap_or(current.cache_creation_input_tokens),
        };
    }

    fn count_kept_blocks_before(&self, index: usize) -> usize {
        self.blocks
            .range(..index)
            .filter(|(_, block)| !matches!(block, BlockState::Ignored))
            .count()
    }

    fn finish(self) -> Result<Completion, Error> {
        if !self.is_stopped {
            return Err(stream_error(
                "the stream ended before `message_stop`".into(),
            ));
        }
        let content = self
            .blocks
            .into_values()
            .map(BlockState::into_response)
            .collect::<Result<Vec<ContentBlockResponse>, Error>>()?;
        build_completion(content, self.stop_reason.as_deref(), self.usage)
    }
}

impl BlockState {
    fn into_response(self) -> Result<ContentBlockResponse, Error> {
        match self {
            BlockState::Text { text } => Ok(ContentBlockResponse::Text { text }),
            BlockState::ToolUse {
                id,
                name,
                input_json,
            } => Ok(ContentBlockResponse::ToolUse {
                id,
                name,
                input: parse_tool_input(&input_json)?,
            }),
            BlockState::Ignored => Ok(ContentBlockResponse::Other),
        }
    }
}

fn report_text(index: usize, text: &str, on_event: &StreamSink<'_>) {
    if text.is_empty() {
        return;
    }
    on_event(StreamEvent::TextDelta {
        index,
        text: text.to_string(),
    });
}

fn parse_tool_input(input_json: &str) -> Result<Value, Error> {
    if input_json.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(input_json)
        .map_err(|error| stream_error(format!("unreadable tool input: {error}")))
}

fn provider_error(error: &ErrorResponse, payload: &str) -> Error {
    Error::Provider {
        provider: PROVIDER_NAME,
        status: map_error_status(&error.kind),
        body: payload.to_string(),
    }
}

fn map_error_status(kind: &str) -> u16 {
    match kind {
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        "billing_error" => 402,
        "permission_error" => 403,
        "not_found_error" => 404,
        "request_too_large" => 413,
        "rate_limit_error" => 429,
        "timeout_error" => 504,
        "overloaded_error" => 529,
        _ => 500,
    }
}

fn stream_error(reason: String) -> Error {
    Error::Stream {
        provider: PROVIDER_NAME,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ContentBlock;
    use crate::message::ToolUse;
    use crate::model::StopReason;
    use crate::model::provider::anthropic::MessagesResponse;
    use crate::model::provider::anthropic::parse_completion;
    use serde_json::json;
    use std::sync::Mutex;

    const TOOL_USE_STREAM: &str = "\
event: message_start
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\
\"role\":\"assistant\",\"model\":\"m\",\"content\":[],\"stop_reason\":null,\
\"usage\":{\"input_tokens\":472,\"output_tokens\":2,\"cache_read_input_tokens\":7,\
\"cache_creation_input_tokens\":11}}}

event: content_block_start
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\
\"thinking\":\"\"}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\
\"thinking\":\"\"}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\
\"signature\":\"abc\"}}

event: content_block_stop
data: {\"type\":\"content_block_stop\",\"index\":0}

event: content_block_start
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}

event: ping
data: {\"type\": \"ping\"}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Okay\"}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\", let's check\"}}

event: content_block_stop
data: {\"type\":\"content_block_stop\",\"index\":1}

event: content_block_start
data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\
\"id\":\"toolu_1\",\"name\":\"get_weather\",\"input\":{}}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\"}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\
\"partial_json\":\"{\\\"location\\\": \\\"San\"}}

event: content_block_delta
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\
\"partial_json\":\" Francisco\\\"}\"}}

event: content_block_stop
data: {\"type\":\"content_block_stop\",\"index\":2}

event: message_delta
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\
\"usage\":{\"output_tokens\":89}}

event: message_stop
data: {\"type\":\"message_stop\"}

";

    fn accumulate(
        stream: &str,
        chunk_size: usize,
    ) -> (Result<Completion, Error>, Vec<StreamEvent>) {
        let events = Mutex::new(Vec::new());
        let on_event = |event: StreamEvent| events.lock().unwrap().push(event);
        let mut decoder = SseDecoder::default();
        let mut accumulator = StreamAccumulator::default();
        for chunk in stream.as_bytes().chunks(chunk_size) {
            for payload in decoder.push(chunk) {
                if let Err(error) = accumulator.apply_payload(&payload, &on_event) {
                    return (Err(error), events.into_inner().unwrap());
                }
            }
        }
        (accumulator.finish(), events.into_inner().unwrap())
    }

    #[test]
    fn reports_deltas_in_order_with_final_message_indexes() {
        let (_, events) = accumulate(TOOL_USE_STREAM, 7);
        assert_eq!(
            events,
            [
                StreamEvent::TextDelta {
                    index: 0,
                    text: "Okay".to_string(),
                },
                StreamEvent::TextDelta {
                    index: 0,
                    text: ", let's check".to_string(),
                },
                StreamEvent::ToolUseStarted {
                    index: 1,
                    id: "toolu_1".to_string(),
                    name: "get_weather".to_string(),
                },
            ]
        );
    }

    #[test]
    fn builds_the_same_completion_as_the_non_streaming_response() {
        let (streamed, _) = accumulate(TOOL_USE_STREAM, 13);
        let response: MessagesResponse = serde_json::from_value(json!({
            "content": [
                { "type": "thinking", "thinking": "", "signature": "abc" },
                { "type": "text", "text": "Okay, let's check" },
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "get_weather",
                    "input": { "location": "San Francisco" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 472,
                "output_tokens": 89,
                "cache_read_input_tokens": 7,
                "cache_creation_input_tokens": 11
            }
        }))
        .unwrap();
        let streamed = streamed.unwrap();
        assert_eq!(streamed, parse_completion(response).unwrap());
        assert_eq!(streamed.stop_reason, StopReason::ToolUse);
        assert_eq!(
            streamed.content[1],
            ContentBlock::ToolUse(ToolUse {
                id: "toolu_1".to_string(),
                name: "get_weather".to_string(),
                input: json!({ "location": "San Francisco" }),
            })
        );
    }

    #[test]
    fn takes_cumulative_usage_from_the_last_message_delta() {
        let stream = "\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}

data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}

data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}

data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\
\"usage\":{\"output_tokens\":3,\"cache_read_input_tokens\":4}}

data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":9}}

data: {\"type\":\"message_stop\"}

";
        let completion = accumulate(stream, 1000).0.unwrap();
        assert_eq!(completion.stop_reason, StopReason::EndTurn);
        assert_eq!(
            completion.usage,
            Usage {
                input_tokens: 5,
                output_tokens: 9,
                cache_read_input_tokens: 4,
                cache_creation_input_tokens: 0,
            }
        );
    }

    #[test]
    fn maps_error_events_to_retryable_provider_errors() {
        let stream = "\
event: error
data: {\"type\": \"error\", \"error\": {\"type\": \"overloaded_error\", \"message\": \"Overloaded\"}}

";
        let error = accumulate(stream, 1000).0.unwrap_err();
        assert!(matches!(error, Error::Provider { status: 529, .. }));
        assert!(error.is_retryable());
    }

    #[test]
    fn rejects_a_stream_cut_before_message_stop() {
        let cut = &TOOL_USE_STREAM[..TOOL_USE_STREAM.find("event: message_delta").unwrap()];
        let error = accumulate(cut, 1000).0.unwrap_err();
        assert!(matches!(error, Error::Stream { .. }));
        assert!(error.is_retryable());
    }

    #[test]
    fn ignores_unknown_events() {
        let stream = "\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}

data: {\"type\":\"brand_new_event\",\"payload\":1}

data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}

data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"brand_new_delta\"}}

data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}

data: {\"type\":\"message_stop\"}

";
        assert_eq!(accumulate(stream, 1000).0.unwrap().text(), "ok");
    }
}
