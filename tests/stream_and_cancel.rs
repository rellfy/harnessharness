use async_trait::async_trait;
use harnessharness::CancelToken;
use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::RetryPolicy;
use harnessharness::message::ContentBlock;
use harnessharness::message::Message;
use harnessharness::message::Role;
use harnessharness::message::ToolResult;
use harnessharness::message::ToolUse;
use harnessharness::model::Completion;
use harnessharness::model::CompletionRequest;
use harnessharness::model::Model;
use harnessharness::model::StopReason;
use harnessharness::model::StreamEvent;
use harnessharness::model::StreamSink;
use harnessharness::model::Usage;
use harnessharness::session::SessionStore;
use harnessharness::tool::Tool;
use harnessharness::tool::ToolContext;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::pending;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

const INTERRUPTED_TOOL_RESULT: &str = "the tool call was cancelled before it returned a result";

#[derive(Clone)]
enum Step {
    Reply(Completion),
    StallAfterDelta(&'static str),
    FailAfterDelta(&'static str),
}

#[derive(Clone, Default)]
struct StreamingModel {
    steps: Arc<Mutex<VecDeque<Step>>>,
    request_drops: Arc<AtomicU32>,
}

struct DropFlag(Arc<AtomicBool>);

struct DropCounter(Arc<AtomicU32>);

#[derive(Debug, Clone, PartialEq)]
enum Observed {
    Stream(StreamEvent),
    Message(Message),
}

struct StallingTool {
    cancel: CancelToken,
    is_dropped: Arc<AtomicBool>,
}

struct Echo;

#[derive(Default)]
struct MemoryStore {
    messages: Mutex<Vec<Message>>,
}

#[async_trait]
impl Model for StreamingModel {
    fn name(&self) -> &str {
        "streaming"
    }

    async fn complete(&self, _request: CompletionRequest<'_>) -> Result<Completion, Error> {
        match self.next_step() {
            Step::Reply(completion) => Ok(completion),
            Step::StallAfterDelta(_) => pending().await,
            Step::FailAfterDelta(_) => Err(retryable_error()),
        }
    }

    async fn stream(
        &self,
        _request: CompletionRequest<'_>,
        on_event: &StreamSink<'_>,
    ) -> Result<Completion, Error> {
        match self.next_step() {
            Step::Reply(completion) => {
                stream_word_by_word(&completion, on_event);
                Ok(completion)
            }
            Step::StallAfterDelta(text) => {
                let _guard = DropCounter(self.request_drops.clone());
                on_event(text_delta(0, text));
                pending().await
            }
            Step::FailAfterDelta(text) => {
                on_event(text_delta(0, text));
                Err(retryable_error())
            }
        }
    }
}

impl StreamingModel {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
            request_drops: Arc::default(),
        }
    }

    fn next_step(&self) -> Step {
        let step = self.steps.lock().unwrap().pop_front();
        step.unwrap_or_else(|| Step::Reply(text_completion("fallback")))
    }
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Tool for StallingTool {
    type Output = String;
    type Error = Infallible;

    fn name(&self) -> &str {
        "stall"
    }

    fn description(&self) -> &str {
        "Never return."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _input: Value, _context: &ToolContext) -> Result<String, Infallible> {
        let _guard = DropFlag(self.is_dropped.clone());
        self.cancel.cancel();
        pending().await
    }
}

impl Tool for Echo {
    type Output = String;
    type Error = Infallible;

    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo the input text."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(&self, input: Value, _context: &ToolContext) -> Result<String, Infallible> {
        Ok(input["text"].as_str().unwrap_or_default().to_string())
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn load(&self, _session_id: &str) -> Result<Option<Vec<Message>>, Error> {
        Ok(Some(self.messages.lock().unwrap().clone()))
    }

    async fn append(&self, _session_id: &str, message: &Message) -> Result<(), Error> {
        self.messages.lock().unwrap().push(message.clone());
        Ok(())
    }
}

#[tokio::test]
async fn streams_deltas_before_the_complete_message() {
    let model = StreamingModel::new([
        Step::Reply(tool_use_completion("checking now", "a")),
        Step::Reply(text_completion("all done here")),
    ]);
    let observed: Arc<Mutex<Vec<Observed>>> = Arc::default();
    let harness = observing(HarnessHarness::new().model(model).tool(Echo), &observed)
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    agent.prompt("go").await.unwrap();
    let observed = observed.lock().unwrap().clone();
    let expected = [
        Observed::Message(Message::user("go")),
        Observed::Stream(StreamEvent::ResponseStarted),
        Observed::Stream(text_delta(0, "checking")),
        Observed::Stream(text_delta(0, " now")),
        Observed::Stream(StreamEvent::ToolUseStarted {
            index: 1,
            id: "a".to_string(),
            name: "echo".to_string(),
        }),
        Observed::Message(agent.messages()[1].clone()),
        Observed::Message(agent.messages()[2].clone()),
        Observed::Stream(StreamEvent::ResponseStarted),
        Observed::Stream(text_delta(0, "all")),
        Observed::Stream(text_delta(0, " done")),
        Observed::Stream(text_delta(0, " here")),
        Observed::Message(Message::assistant("all done here")),
    ];
    assert_eq!(observed, expected);
}

#[tokio::test]
async fn keeps_the_same_history_with_and_without_streaming() {
    let steps = [
        Step::Reply(tool_use_completion("checking", "a")),
        Step::Reply(text_completion("done")),
    ];
    let plain = HarnessHarness::new()
        .model(StreamingModel::new(steps.clone()))
        .tool(Echo)
        .build()
        .unwrap();
    let streamed = HarnessHarness::new()
        .model(StreamingModel::new(steps))
        .tool(Echo)
        .on_stream(|_, _| {})
        .build()
        .unwrap();
    let mut plain_agent = plain.spawn();
    let mut streamed_agent = streamed.spawn();
    let plain_output = plain_agent.prompt("go").await.unwrap();
    let streamed_output = streamed_agent.prompt("go").await.unwrap();
    assert_eq!(plain_output, streamed_output);
    assert_eq!(plain_agent.messages(), streamed_agent.messages());
    assert_eq!(plain_agent.messages().len(), 4);
}

#[tokio::test]
async fn restarts_the_stream_on_retry() {
    let model = StreamingModel::new([
        Step::FailAfterDelta("partial"),
        Step::Reply(text_completion("whole")),
    ]);
    let observed: Arc<Mutex<Vec<Observed>>> = Arc::default();
    let retry = RetryPolicy::default().initial_delay(Duration::ZERO);
    let harness = observing(HarnessHarness::new().model(model).retry(retry), &observed)
        .build()
        .unwrap();
    harness.spawn().prompt("go").await.unwrap();
    let streamed: Vec<Observed> = observed
        .lock()
        .unwrap()
        .iter()
        .filter(|observed| matches!(observed, Observed::Stream(_)))
        .cloned()
        .collect();
    let expected = [
        Observed::Stream(StreamEvent::ResponseStarted),
        Observed::Stream(text_delta(0, "partial")),
        Observed::Stream(StreamEvent::ResponseStarted),
        Observed::Stream(text_delta(0, "whole")),
    ];
    assert_eq!(streamed, expected);
}

#[tokio::test]
async fn cancels_mid_stream_without_keeping_the_partial_message() {
    let model = StreamingModel::new([
        Step::StallAfterDelta("half a tho"),
        Step::Reply(text_completion("fresh answer")),
    ]);
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    let store = Arc::new(MemoryStore::default());
    let harness = HarnessHarness::new()
        .model(model.clone())
        .session_store(SharedStore(store.clone()))
        .on_stream(move |_, event| {
            if matches!(event, StreamEvent::TextDelta { .. }) {
                canceller.cancel();
            }
        })
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    let result = agent.prompt_cancellable("first", &cancel).await;
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(model.request_drops.load(Ordering::SeqCst), 1);
    assert_eq!(agent.messages(), [Message::user("first")]);
    assert_eq!(*store.messages.lock().unwrap(), agent.messages());
    assert_eq!(agent.prompt("second").await.unwrap(), "fresh answer");
    let expected = [
        Message::user("first"),
        Message::user("second"),
        Message::assistant("fresh answer"),
    ];
    assert_eq!(agent.messages(), expected);
}

#[tokio::test]
async fn cancels_mid_tool_and_answers_the_tool_call() {
    let cancel = CancelToken::new();
    let is_dropped = Arc::new(AtomicBool::new(false));
    let tool = StallingTool {
        cancel: cancel.clone(),
        is_dropped: is_dropped.clone(),
    };
    let model = StreamingModel::new([
        Step::Reply(tool_use_completion_named("running", "a", "stall")),
        Step::Reply(text_completion("after")),
    ]);
    let store = Arc::new(MemoryStore::default());
    let harness = HarnessHarness::new()
        .model(model)
        .tool(tool)
        .session_store(SharedStore(store.clone()))
        .on_stream(|_, _| {})
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    let result = agent.prompt_cancellable("go", &cancel).await;
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(is_dropped.load(Ordering::SeqCst));
    assert_eq!(agent.messages().len(), 3);
    assert_eq!(agent.messages()[2], interrupted_results(&["a"]));
    assert_eq!(*store.messages.lock().unwrap(), agent.messages());
    assert_eq!(agent.prompt("again").await.unwrap(), "after");
    assert_eq!(agent.messages()[3], Message::user("again"));
}

#[tokio::test]
async fn answers_dangling_tool_calls_of_a_restored_session() {
    let model = StreamingModel::new([Step::Reply(text_completion("resumed"))]);
    let harness = HarnessHarness::new().model(model).build().unwrap();
    let history = vec![
        Message::user("go"),
        tool_use_message("checking", &[("a", "echo"), ("b", "echo")]),
    ];
    let mut agent = harnessharness::Agent::restore(harness, "id".to_string(), history);
    agent.prompt("next").await.unwrap();
    let messages = agent.messages();
    assert_eq!(messages[2], interrupted_results(&["a", "b"]));
    assert_eq!(messages[3], Message::user("next"));
    assert_eq!(messages[4], Message::assistant("resumed"));
}

#[tokio::test]
async fn ignores_a_cancel_after_the_prompt_finished() {
    let model = StreamingModel::new([Step::Reply(text_completion("done"))]);
    let harness = HarnessHarness::new().model(model).build().unwrap();
    let cancel = CancelToken::new();
    let mut agent = harness.spawn();
    assert_eq!(
        agent.prompt_cancellable("go", &cancel).await.unwrap(),
        "done"
    );
    cancel.cancel();
    assert_eq!(agent.messages().len(), 2);
}

struct SharedStore(Arc<MemoryStore>);

#[async_trait]
impl SessionStore for SharedStore {
    async fn load(&self, session_id: &str) -> Result<Option<Vec<Message>>, Error> {
        self.0.load(session_id).await
    }

    async fn append(&self, session_id: &str, message: &Message) -> Result<(), Error> {
        self.0.append(session_id, message).await
    }
}

fn observing(builder: HarnessHarness, observed: &Arc<Mutex<Vec<Observed>>>) -> HarnessHarness {
    let stream_log = observed.clone();
    let message_log = observed.clone();
    builder
        .on_stream(move |_, event| {
            stream_log
                .lock()
                .unwrap()
                .push(Observed::Stream(event.clone()))
        })
        .on_message(move |_, message| {
            message_log
                .lock()
                .unwrap()
                .push(Observed::Message(message.clone()))
        })
}

fn stream_word_by_word(completion: &Completion, on_event: &StreamSink<'_>) {
    for (index, block) in completion.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text } => {
                for word in split_words(text) {
                    on_event(text_delta(index, word));
                }
            }
            ContentBlock::ToolUse(tool_use) => on_event(StreamEvent::ToolUseStarted {
                index,
                id: tool_use.id.clone(),
                name: tool_use.name.clone(),
            }),
            ContentBlock::ToolResult(_) => {}
        }
    }
}

fn split_words(text: &str) -> Vec<&str> {
    let starts = text
        .char_indices()
        .filter(|(index, character)| *index == 0 || *character == ' ')
        .map(|(index, _)| index)
        .chain([text.len()])
        .collect::<Vec<usize>>();
    starts
        .windows(2)
        .map(|pair| &text[pair[0]..pair[1]])
        .collect()
}

fn text_delta(index: usize, text: &str) -> StreamEvent {
    StreamEvent::TextDelta {
        index,
        text: text.to_string(),
    }
}

fn text_completion(text: &str) -> Completion {
    Completion {
        content: vec![ContentBlock::text(text)],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

fn tool_use_completion(text: &str, id: &str) -> Completion {
    tool_use_completion_named(text, id, "echo")
}

fn tool_use_completion_named(text: &str, id: &str, name: &str) -> Completion {
    Completion {
        content: tool_use_message(text, &[(id, name)]).content,
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn tool_use_message(text: &str, tool_uses: &[(&str, &str)]) -> Message {
    let tool_blocks = tool_uses.iter().map(|(id, name)| {
        ContentBlock::ToolUse(ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({ "text": "hi" }),
        })
    });
    let content = [ContentBlock::text(text)]
        .into_iter()
        .chain(tool_blocks)
        .collect();
    Message::new(Role::Assistant, content)
}

fn interrupted_results(ids: &[&str]) -> Message {
    let results = ids
        .iter()
        .map(|id| ToolResult {
            tool_use_id: id.to_string(),
            content: INTERRUPTED_TOOL_RESULT.to_string(),
            is_error: true,
        })
        .collect();
    Message::tool_results(results)
}

fn retryable_error() -> Error {
    Error::Provider {
        provider: "streaming",
        status: 529,
        body: String::new(),
    }
}
