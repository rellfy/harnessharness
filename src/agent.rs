use crate::cancel::CancelToken;
use crate::compaction::CompactionPolicy;
use crate::compaction::SUMMARY_REQUEST;
use crate::compaction::build_context;
use crate::compaction::estimate_prompt_tokens;
use crate::compaction::estimate_tokens;
use crate::compaction::find_split;
use crate::error::Error;
use crate::harness::Harness;
use crate::message::Compaction;
use crate::message::Message;
use crate::message::Role;
use crate::message::ToolResult;
use crate::message::ToolUse;
use crate::model::Completion;
use crate::model::CompletionRequest;
use crate::model::StreamEvent;
use crate::model::Usage;
use crate::retry::retry;
use crate::tool::ToolContext;
use crate::tool::ToolOutput;
use crate::tracer::span::AgentSpan;
use futures_util::future::join_all;
use uuid::Uuid;

const INTERRUPTED_TOOL_RESULT: &str = "the tool call was cancelled before it returned a result";

pub struct Agent {
    id: String,
    harness: Harness,
    messages: Vec<Message>,
    measured_prompt: Option<MeasuredPrompt>,
}

/// The provider-reported size of the last prompt, and how many context messages it held.
#[derive(Debug, Clone, Copy)]
struct MeasuredPrompt {
    context_len: usize,
    tokens: u64,
}

impl Agent {
    pub fn new(harness: Harness) -> Self {
        Self::restore(harness, Uuid::now_v7().to_string(), Vec::new())
    }

    pub fn restore(harness: Harness, id: String, messages: Vec<Message>) -> Self {
        Self {
            id,
            harness,
            messages,
            measured_prompt: None,
        }
    }

    pub async fn prompt(&mut self, input: impl Into<String>) -> Result<String, Error> {
        self.prompt_cancellable(input, &CancelToken::new()).await
    }

    /// Like `prompt`, but stops with `Error::Cancelled` once `cancel` is cancelled.
    /// The in-flight model request or tool calls are dropped.
    /// The history stays valid: a cancelled response is not kept, and tool calls that were
    /// running get error results.
    pub async fn prompt_cancellable(
        &mut self,
        input: impl Into<String>,
        cancel: &CancelToken,
    ) -> Result<String, Error> {
        let input = input.into();
        let span = AgentSpan::start(&self.harness.tracer, "prompt", &self.id, &input);
        let result = self.run_loop(&span, input, cancel).await;
        span.end(result)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The full history, including messages that compaction has hidden from the model.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The messages the model sees next.
    pub fn context(&self) -> Vec<Message> {
        build_context(&self.messages).into_owned()
    }

    async fn run_loop(
        &mut self,
        span: &AgentSpan,
        input: String,
        cancel: &CancelToken,
    ) -> Result<String, Error> {
        self.close_dangling_tool_uses().await?;
        self.push_message(Message::user(input)).await?;
        for _ in 0..self.harness.max_iterations {
            self.compact_if_needed(span, cancel).await?;
            let completion = cancel
                .guard(self.complete(span))
                .await
                .ok_or(Error::Cancelled)??;
            self.measure_prompt(&completion.usage);
            let message = Message::new(Role::Assistant, completion.content);
            let tool_uses: Vec<ToolUse> = message.tool_uses().into_iter().cloned().collect();
            let text = message.text();
            self.push_message(message).await?;
            if tool_uses.is_empty() {
                return Ok(text);
            }
            let tool_results = cancel.guard(self.execute_tools(span, &tool_uses)).await;
            let Some(tool_results) = tool_results else {
                self.close_dangling_tool_uses().await?;
                return Err(Error::Cancelled);
            };
            self.push_message(Message::tool_results(tool_results))
                .await?;
        }
        Err(Error::MaxIterationsExceeded(self.harness.max_iterations))
    }

    async fn complete(&self, span: &AgentSpan) -> Result<Completion, Error> {
        let tracer = &self.harness.tracer;
        let model = &self.harness.model;
        let context = build_context(&self.messages);
        let request = CompletionRequest {
            instructions: &self.harness.instructions,
            messages: &context,
            tools: &self.harness.tool_specs,
        };
        retry(&self.harness.retry, || async {
            let generation = span.generation(tracer, model.name(), &request);
            let completion_result = self.request_completion(request.clone()).await;
            generation.end(completion_result)
        })
        .await
    }

    async fn request_completion(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<Completion, Error> {
        let Some(stream_listener) = &self.harness.stream_listener else {
            return self.harness.model.complete(request).await;
        };
        stream_listener(&self.id, &StreamEvent::ResponseStarted);
        let on_event = |event: StreamEvent| stream_listener(&self.id, &event);
        self.harness.model.stream(request, &on_event).await
    }

    async fn compact_if_needed(
        &mut self,
        span: &AgentSpan,
        cancel: &CancelToken,
    ) -> Result<(), Error> {
        let Some(policy) = self.harness.compaction else {
            return Ok(());
        };
        let Some(split) = self.find_compaction_split(&policy) else {
            return Ok(());
        };
        let summary = cancel
            .guard(self.summarise(span, split))
            .await
            .ok_or(Error::Cancelled)??;
        let kept_message_count = build_context(&self.messages).len() - split;
        let compaction = Compaction {
            summary,
            kept_message_count,
        };
        self.push_message(Message::compaction(compaction)).await?;
        self.measured_prompt = None;
        Ok(())
    }

    fn find_compaction_split(&self, policy: &CompactionPolicy) -> Option<usize> {
        let context = build_context(&self.messages);
        if self.estimate_context_tokens(&context) < policy.trigger_tokens {
            return None;
        }
        find_split(&context, policy.keep_tokens)
    }

    fn estimate_context_tokens(&self, context: &[Message]) -> u64 {
        match self.measured_prompt {
            Some(measured) if measured.context_len <= context.len() => {
                measured.tokens + estimate_tokens(&context[measured.context_len..])
            }
            _ => {
                let prompt_tokens =
                    estimate_prompt_tokens(&self.harness.instructions, &self.harness.tool_specs);
                prompt_tokens + estimate_tokens(context)
            }
        }
    }

    /// Asks the model to summarise `context[..split]`.
    /// The request repeats the regular prefix (tools, instructions, messages) so it can reuse the
    /// prompt cache, and is not streamed to the embedder.
    async fn summarise(&self, span: &AgentSpan, split: usize) -> Result<String, Error> {
        let tracer = &self.harness.tracer;
        let model = &self.harness.model;
        let context = build_context(&self.messages);
        let messages: Vec<Message> = context[..split]
            .iter()
            .cloned()
            .chain([Message::user(SUMMARY_REQUEST)])
            .collect();
        let request = CompletionRequest {
            instructions: &self.harness.instructions,
            messages: &messages,
            tools: &self.harness.tool_specs,
        };
        let completion = retry(&self.harness.retry, || async {
            let generation = span.generation(tracer, model.name(), &request);
            generation.end(model.complete(request.clone()).await)
        })
        .await?;
        let summary = completion.text();
        match summary.trim().is_empty() {
            true => Err(Error::EmptyResponse),
            false => Ok(summary.trim().to_string()),
        }
    }

    fn measure_prompt(&mut self, usage: &Usage) {
        let tokens =
            usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
        self.measured_prompt = match tokens {
            0 => None,
            _ => Some(MeasuredPrompt {
                context_len: build_context(&self.messages).len(),
                tokens,
            }),
        };
    }

    async fn execute_tools(&self, span: &AgentSpan, tool_uses: &[ToolUse]) -> Vec<ToolResult> {
        let context = ToolContext::new(&self.id);
        let executions = tool_uses
            .iter()
            .map(|tool_use| self.execute_tool(span, &context, tool_use));
        join_all(executions).await
    }

    /// The single chokepoint for tool calls; policy and hooks belong here.
    async fn execute_tool(
        &self,
        span: &AgentSpan,
        context: &ToolContext,
        tool_use: &ToolUse,
    ) -> ToolResult {
        let tool_span = span.tool(&self.harness.tracer, tool_use);
        let result = match self.harness.find_tool(&tool_use.name) {
            Some(tool) => tool.call(tool_use.input.clone(), context).await,
            None => Err(format!("unknown tool `{}`", tool_use.name)),
        };
        tool_span.end(build_tool_result(&tool_use.id, result))
    }

    /// Answer tool calls left without results by a cancel, a crash or a failed append, so the
    /// history the model sees next pairs every tool call with a result.
    async fn close_dangling_tool_uses(&mut self) -> Result<(), Error> {
        let Some(last_message) = self.messages.last() else {
            return Ok(());
        };
        if last_message.role != Role::Assistant {
            return Ok(());
        }
        let tool_results: Vec<ToolResult> = last_message
            .tool_uses()
            .into_iter()
            .map(|tool_use| build_tool_result(&tool_use.id, Err(INTERRUPTED_TOOL_RESULT.into())))
            .collect();
        if tool_results.is_empty() {
            return Ok(());
        }
        self.push_message(Message::tool_results(tool_results)).await
    }

    async fn push_message(&mut self, message: Message) -> Result<(), Error> {
        if let Some(session_store) = &self.harness.session_store {
            session_store.append(&self.id, &message).await?;
        }
        if let Some(message_listener) = &self.harness.message_listener {
            message_listener(&self.id, &message);
        }
        self.messages.push(message);
        Ok(())
    }
}

fn build_tool_result(tool_use_id: &str, result: Result<ToolOutput, String>) -> ToolResult {
    let is_error = result.is_err();
    let content = match result {
        Ok(output) => output.into_content(),
        Err(message) => message,
    };
    ToolResult {
        tool_use_id: tool_use_id.to_string(),
        content,
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::HarnessHarness;
    use crate::message::ContentBlock;
    use crate::model::Model;
    use crate::model::StopReason;
    use crate::model::Usage;
    use crate::tool::Tool;
    use async_trait::async_trait;
    use serde_json::Value;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct ScriptedModel {
        completions: Arc<Mutex<VecDeque<Completion>>>,
        tool_names_seen: Arc<Mutex<Vec<String>>>,
    }

    struct Echo;

    struct Failing;

    #[async_trait]
    impl Model for ScriptedModel {
        fn name(&self) -> &str {
            "scripted"
        }

        async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, Error> {
            let tool_names = request.tools.iter().map(|tool| tool.name.clone());
            *self.tool_names_seen.lock().unwrap() = tool_names.collect();
            let completion = self.completions.lock().unwrap().pop_front();
            Ok(completion.unwrap_or_else(|| tool_use_completion(&[("loop", "echo")])))
        }
    }

    impl ScriptedModel {
        fn new(completions: impl IntoIterator<Item = Completion>) -> Self {
            Self {
                completions: Arc::new(Mutex::new(completions.into_iter().collect())),
                tool_names_seen: Arc::default(),
            }
        }
    }

    impl Tool for Echo {
        type Output = String;
        type Error = Infallible;

        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echo the input text with the agent id."
        }

        fn input_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn call(&self, input: Value, context: &ToolContext) -> Result<String, Infallible> {
            let text = input["text"].as_str().unwrap_or_default();
            Ok(format!("{text}@{}", context.agent_id()))
        }
    }

    impl Tool for Failing {
        type Output = ();
        type Error = &'static str;

        fn name(&self) -> &str {
            "failing"
        }

        fn description(&self) -> &str {
            "Always fail."
        }

        fn input_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn call(&self, _input: Value, _context: &ToolContext) -> Result<(), &'static str> {
            Err("boom")
        }
    }

    fn text_completion(text: &str) -> Completion {
        Completion {
            content: vec![ContentBlock::text(text)],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }
    }

    fn tool_use_completion(tool_uses: &[(&str, &str)]) -> Completion {
        let content = tool_uses
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolUse(ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: json!({ "text": "hi" }),
                })
            })
            .collect();
        Completion {
            content,
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        }
    }

    fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> ToolResult {
        ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content.to_string(),
            is_error,
        }
    }

    #[tokio::test]
    async fn returns_text_without_tools() {
        let model = ScriptedModel::new([text_completion("hello")]);
        let harness = HarnessHarness::new().model(model).build().unwrap();
        let mut agent = harness.spawn();
        assert_eq!(agent.prompt("hi").await.unwrap(), "hello");
        assert_eq!(
            agent.messages(),
            [Message::user("hi"), Message::assistant("hello")]
        );
    }

    #[tokio::test]
    async fn notifies_message_listener_in_order() {
        let model = ScriptedModel::new([
            tool_use_completion(&[("a", "echo")]),
            text_completion("done"),
        ]);
        let seen: Arc<Mutex<Vec<(String, Message)>>> = Arc::default();
        let seen_by_listener = seen.clone();
        let harness = HarnessHarness::new()
            .model(model)
            .tool(Echo)
            .on_message(move |agent_id, message| {
                let entry = (agent_id.to_string(), message.clone());
                seen_by_listener.lock().unwrap().push(entry);
            })
            .build()
            .unwrap();
        let mut agent = harness.spawn();
        agent.prompt("go").await.unwrap();
        let seen = seen.lock().unwrap();
        let seen_messages: Vec<Message> = seen.iter().map(|(_, message)| message.clone()).collect();
        assert_eq!(seen_messages, agent.messages());
        assert!(seen.iter().all(|(agent_id, _)| agent_id == agent.id()));
    }

    #[tokio::test]
    async fn sends_tool_specs_to_model() {
        let model = ScriptedModel::new([text_completion("hello")]);
        let harness = HarnessHarness::new()
            .model(model.clone())
            .tool(Echo)
            .tool(Failing)
            .build()
            .unwrap();
        harness.spawn().prompt("hi").await.unwrap();
        assert_eq!(*model.tool_names_seen.lock().unwrap(), ["echo", "failing"]);
    }

    #[tokio::test]
    async fn feeds_parallel_tool_results_back_in_one_message() {
        let model = ScriptedModel::new([
            tool_use_completion(&[("a", "echo"), ("b", "failing"), ("c", "missing")]),
            text_completion("done"),
        ]);
        let harness = HarnessHarness::new()
            .model(model)
            .tools([Echo])
            .tool(Failing)
            .build()
            .unwrap();
        let mut agent = harness.spawn();
        assert_eq!(agent.prompt("go").await.unwrap(), "done");
        let expected_results = Message::tool_results(vec![
            tool_result("a", &format!("hi@{}", agent.id()), false),
            tool_result("b", "boom", true),
            tool_result("c", "unknown tool `missing`", true),
        ]);
        assert_eq!(agent.messages().len(), 4);
        assert_eq!(agent.messages()[2], expected_results);
    }

    #[tokio::test]
    async fn stops_at_max_iterations() {
        let harness = HarnessHarness::new()
            .model(ScriptedModel::default())
            .tool(Echo)
            .max_iterations(3)
            .build()
            .unwrap();
        let result = harness.spawn().prompt("go").await;
        assert!(matches!(result, Err(Error::MaxIterationsExceeded(3))));
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let result = HarnessHarness::new()
            .model(ScriptedModel::default())
            .tools([Echo, Echo])
            .build();
        assert!(matches!(result, Err(Error::DuplicateTool(name)) if name == "echo"));
    }
}
