use crate::error::Error;
use crate::message::Message;
use crate::model::Completion;
use crate::model::CompletionRequest;
use opentelemetry::Context;
use opentelemetry::KeyValue;
use opentelemetry::global::BoxedSpan;
use opentelemetry::global::BoxedTracer;
use opentelemetry::trace::Span;
use opentelemetry::trace::SpanKind;
use opentelemetry::trace::Status;
use opentelemetry::trace::TraceContextExt;
use opentelemetry::trace::Tracer;
use serde::Serialize;
use serde_json::json;

pub const OBSERVATION_TYPE: &str = "langfuse.observation.type";
pub const OBSERVATION_INPUT: &str = "langfuse.observation.input";
pub const OBSERVATION_OUTPUT: &str = "langfuse.observation.output";
pub const OBSERVATION_MODEL_NAME: &str = "langfuse.observation.model.name";
pub const OBSERVATION_USAGE_DETAILS: &str = "langfuse.observation.usage_details";
pub const OBSERVATION_LEVEL: &str = "langfuse.observation.level";
pub const OBSERVATION_STATUS_MESSAGE: &str = "langfuse.observation.status_message";
pub const SESSION_ID: &str = "langfuse.session.id";
pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
pub const GEN_AI_USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";

pub struct AgentSpan {
    span: BoxedSpan,
}

pub struct GenerationSpan {
    span: BoxedSpan,
}

#[derive(Serialize)]
struct InstructionsMessage<'a> {
    role: &'static str,
    content: &'a str,
}

impl AgentSpan {
    pub fn start(tracer: &BoxedTracer, name: &'static str, session_id: &str, input: &str) -> Self {
        let span = tracer
            .span_builder(name)
            .with_kind(SpanKind::Internal)
            .with_attributes([
                KeyValue::new(OBSERVATION_TYPE, "agent"),
                KeyValue::new(SESSION_ID, session_id.to_string()),
                KeyValue::new(OBSERVATION_INPUT, input.to_string()),
            ])
            .start(tracer);
        Self { span }
    }

    pub fn generation(
        &self,
        tracer: &BoxedTracer,
        model_name: &str,
        request: &CompletionRequest<'_>,
    ) -> GenerationSpan {
        let parent_context =
            Context::new().with_remote_span_context(self.span.span_context().clone());
        let span = tracer
            .span_builder("generation")
            .with_kind(SpanKind::Client)
            .with_attributes([
                KeyValue::new(OBSERVATION_TYPE, "generation"),
                KeyValue::new(GEN_AI_OPERATION_NAME, "chat"),
                KeyValue::new(GEN_AI_REQUEST_MODEL, model_name.to_string()),
                KeyValue::new(OBSERVATION_MODEL_NAME, model_name.to_string()),
                KeyValue::new(OBSERVATION_INPUT, serialize_request(request)),
            ])
            .start_with_context(tracer, &parent_context);
        GenerationSpan { span }
    }

    pub fn end(mut self, result: Result<Completion, Error>) -> Result<Completion, Error> {
        match &result {
            Ok(completion) => record_output(&mut self.span, &completion.text),
            Err(error) => record_error(&mut self.span, error),
        }
        self.span.end();
        result
    }
}

impl GenerationSpan {
    pub fn end(mut self, result: Result<Completion, Error>) -> Result<Completion, Error> {
        match &result {
            Ok(completion) => record_completion(&mut self.span, completion),
            Err(error) => record_error(&mut self.span, error),
        }
        self.span.end();
        result
    }
}

fn record_completion(span: &mut BoxedSpan, completion: &Completion) {
    let usage_details = json!({
        "input": completion.usage.input_tokens,
        "output": completion.usage.output_tokens,
    });
    span.set_attributes([
        KeyValue::new(OBSERVATION_OUTPUT, completion.text.clone()),
        KeyValue::new(
            GEN_AI_USAGE_INPUT_TOKENS,
            completion.usage.input_tokens as i64,
        ),
        KeyValue::new(
            GEN_AI_USAGE_OUTPUT_TOKENS,
            completion.usage.output_tokens as i64,
        ),
        KeyValue::new(OBSERVATION_USAGE_DETAILS, usage_details.to_string()),
    ]);
    span.set_status(Status::Ok);
}

fn record_output(span: &mut BoxedSpan, output: &str) {
    span.set_attribute(KeyValue::new(OBSERVATION_OUTPUT, output.to_string()));
    span.set_status(Status::Ok);
}

fn record_error(span: &mut BoxedSpan, error: &Error) {
    let message = error.to_string();
    span.set_attributes([
        KeyValue::new(OBSERVATION_LEVEL, "ERROR"),
        KeyValue::new(OBSERVATION_STATUS_MESSAGE, message.clone()),
    ]);
    span.set_status(Status::error(message));
}

fn serialize_request(request: &CompletionRequest<'_>) -> String {
    let instructions = InstructionsMessage {
        role: "system",
        content: request.instructions,
    };
    let mut input = vec![json!(instructions)];
    input.extend(
        request
            .messages
            .iter()
            .map(|message: &Message| json!(message)),
    );
    json!(input).to_string()
}
