use crate::error::Error;
use crate::harness::Harness;
use crate::message::Message;
use crate::message::Role;
use crate::message::ToolResult;
use crate::message::ToolUse;
use crate::model::Completion;
use crate::model::CompletionRequest;
use crate::retry::retry;
use crate::tool::ToolContext;
use crate::tool::ToolOutput;
use crate::tracer::span::AgentSpan;
use futures_util::future::join_all;
use uuid::Uuid;

pub struct Agent {
    id: String,
    harness: Harness,
    messages: Vec<Message>,
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
        }
    }

    pub async fn prompt(&mut self, input: impl Into<String>) -> Result<String, Error> {
        let input = input.into();
        let span = AgentSpan::start(&self.harness.tracer, "prompt", &self.id, &input);
        let result = self.run_loop(&span, input).await;
        span.end(result)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    async fn run_loop(&mut self, span: &AgentSpan, input: String) -> Result<String, Error> {
        self.push_message(Message::user(input)).await?;
        for _ in 0..self.harness.max_iterations {
            let completion = self.complete(span).await?;
            let message = Message::new(Role::Assistant, completion.content);
            let tool_results = self.execute_tools(span, &message.tool_uses()).await;
            let text = message.text();
            self.push_message(message).await?;
            if tool_results.is_empty() {
                return Ok(text);
            }
            self.push_message(Message::tool_results(tool_results))
                .await?;
        }
        Err(Error::MaxIterationsExceeded(self.harness.max_iterations))
    }

    async fn complete(&self, span: &AgentSpan) -> Result<Completion, Error> {
        let tracer = &self.harness.tracer;
        let model = &self.harness.model;
        let request = CompletionRequest {
            instructions: &self.harness.instructions,
            messages: &self.messages,
            tools: &self.harness.tool_specs,
        };
        retry(&self.harness.retry, || async {
            let generation = span.generation(tracer, model.name(), &request);
            let completion_result = model.complete(request.clone()).await;
            generation.end(completion_result)
        })
        .await
    }

    async fn execute_tools(&self, span: &AgentSpan, tool_uses: &[&ToolUse]) -> Vec<ToolResult> {
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

    async fn push_message(&mut self, message: Message) -> Result<(), Error> {
        if let Some(session_store) = &self.harness.session_store {
            session_store.append(&self.id, &message).await?;
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
