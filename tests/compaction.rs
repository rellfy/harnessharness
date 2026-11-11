use async_trait::async_trait;
use harnessharness::Agent;
use harnessharness::CancelToken;
use harnessharness::CompactionPolicy;
use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::message::ContentBlock;
use harnessharness::message::Message;
use harnessharness::message::Role;
use harnessharness::message::ToolUse;
use harnessharness::model::Completion;
use harnessharness::model::CompletionRequest;
use harnessharness::model::Model;
use harnessharness::model::StopReason;
use harnessharness::model::Usage;
use harnessharness::session::SessionStore;
use harnessharness::tool::Tool;
use harnessharness::tool::ToolContext;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;
use std::convert::Infallible;
use std::future::pending;
use std::sync::Arc;
use std::sync::Mutex;

const SUMMARY_REQUEST_START: &str = "Your context window is nearly full.";
const SUMMARY_PREFIX_START: &str = "This conversation was compacted";
const TOOL_CALLS_PER_PROMPT: usize = 3;
const OUTPUT_BYTES: usize = 400;
const TRIGGER_TOKENS: u64 = 600;
const KEEP_TOKENS: u64 = 200;

#[derive(Clone, Default)]
struct CodingModel {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    summary_count: Arc<Mutex<usize>>,
    is_summary_stalled: bool,
}

struct Echo;

#[derive(Default)]
struct MemoryStore {
    messages: Mutex<Vec<Message>>,
}

struct SharedStore(Arc<MemoryStore>);

#[async_trait]
impl Model for CodingModel {
    fn name(&self) -> &str {
        "coding"
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, Error> {
        let messages = request.messages.to_vec();
        self.requests.lock().unwrap().push(messages.clone());
        let usage = measure(&request);
        if check_is_summary_request(&messages) {
            return self.summarise(usage).await;
        }
        let tool_call_count = count_tool_calls_since_prompt(&messages);
        let completion = match tool_call_count < TOOL_CALLS_PER_PROMPT {
            true => tool_call(&format!("call-{}", messages.len()), usage),
            false => reply("done", usage),
        };
        Ok(completion)
    }
}

impl CodingModel {
    async fn summarise(&self, usage: Usage) -> Result<Completion, Error> {
        if self.is_summary_stalled {
            return pending().await;
        }
        let mut summary_count = self.summary_count.lock().unwrap();
        *summary_count += 1;
        Ok(reply(&format!("summary {summary_count}"), usage))
    }

    fn summary_count(&self) -> usize {
        *self.summary_count.lock().unwrap()
    }

    fn task_requests(&self) -> Vec<Vec<Message>> {
        let requests = self.requests.lock().unwrap();
        requests
            .iter()
            .filter(|messages| !check_is_summary_request(messages))
            .cloned()
            .collect()
    }
}

impl Tool for Echo {
    type Output = String;
    type Error = Infallible;

    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Print a long output."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _input: Value, _context: &ToolContext) -> Result<String, Infallible> {
        Ok("o".repeat(OUTPUT_BYTES))
    }
}

#[async_trait]
impl SessionStore for SharedStore {
    async fn load(&self, _session_id: &str) -> Result<Option<Vec<Message>>, Error> {
        Ok(Some(self.0.messages.lock().unwrap().clone()))
    }

    async fn append(&self, _session_id: &str, message: &Message) -> Result<(), Error> {
        self.0.messages.lock().unwrap().push(message.clone());
        Ok(())
    }
}

#[tokio::test]
async fn keeps_a_long_session_within_the_limit() {
    let model = CodingModel::default();
    let harness = build_harness(&model, Arc::default());
    let mut agent = harness.spawn();
    for prompt in 0..8 {
        assert_eq!(
            agent.prompt(format!("task {prompt}")).await.unwrap(),
            "done"
        );
    }
    assert!(model.summary_count() >= 2, "{}", model.summary_count());
    for messages in model.task_requests() {
        assert_valid_context(&messages);
        let tokens = estimate(&messages);
        assert!(tokens < TRIGGER_TOKENS * 2, "{tokens} tokens");
    }
    let compaction_count = agent
        .messages()
        .iter()
        .filter(|message| message.as_compaction().is_some())
        .count();
    assert_eq!(compaction_count, model.summary_count());
    let prompts_in_history = agent
        .messages()
        .iter()
        .filter(|message| message.text().starts_with("task "))
        .count();
    assert_eq!(prompts_in_history, 8);
}

#[tokio::test]
async fn sends_the_summary_and_the_recent_messages_after_compaction() {
    let model = CodingModel::default();
    let harness = build_harness(&model, Arc::default());
    let mut agent = harness.spawn();
    while model.summary_count() == 0 {
        agent.prompt("work").await.unwrap();
    }
    let record_index = agent
        .messages()
        .iter()
        .rposition(|message| message.as_compaction().is_some())
        .unwrap();
    let compaction = agent.messages()[record_index].as_compaction().unwrap();
    assert_eq!(compaction.summary, "summary 1");
    let context = agent.context();
    assert!(context[0].text().starts_with(SUMMARY_PREFIX_START));
    assert!(context[0].text().ends_with("summary 1"));
    assert_eq!(context[1].role, Role::Assistant);
    let kept = &agent.messages()[record_index - compaction.kept_message_count..record_index];
    assert_eq!(&context[1..=kept.len()], kept);
    let summary_request = model
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|messages| check_is_summary_request(messages))
        .cloned()
        .unwrap();
    let summarised = &summary_request[..summary_request.len() - 1];
    assert_eq!(summarised, &agent.messages()[..summarised.len()]);
}

#[tokio::test]
async fn restores_the_compacted_context_from_the_store() {
    let model = CodingModel::default();
    let store = Arc::new(MemoryStore::default());
    let harness = build_harness(&model, store.clone());
    let mut agent = harness.spawn();
    while model.summary_count() == 0 {
        agent.prompt("work").await.unwrap();
    }
    let stored = store.messages.lock().unwrap().clone();
    assert_eq!(stored, agent.messages());
    let resumed = harness.resume_session(agent.id()).await.unwrap();
    assert_eq!(resumed.context(), agent.context());
    let mut resumed = Agent::restore(harness.clone(), "copy".to_string(), stored);
    resumed.prompt("more").await.unwrap();
    let last_request = model.task_requests().last().cloned().unwrap();
    assert!(last_request[0].text().starts_with(SUMMARY_PREFIX_START));
    assert_valid_context(&last_request);
}

#[tokio::test]
async fn does_not_compact_when_disabled() {
    let model = CodingModel::default();
    let harness = HarnessHarness::new()
        .model(model.clone())
        .tool(Echo)
        .disable_compaction()
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    for _ in 0..4 {
        agent.prompt("work").await.unwrap();
    }
    assert_eq!(model.summary_count(), 0);
    assert_eq!(agent.context(), agent.messages());
}

#[tokio::test]
async fn compacts_by_default_with_a_large_trigger() {
    let model = CodingModel::default();
    let harness = HarnessHarness::new()
        .model(model.clone())
        .tool(Echo)
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    agent.prompt("work").await.unwrap();
    assert_eq!(model.summary_count(), 0);
    assert_eq!(CompactionPolicy::default().trigger_tokens, 150_000);
}

#[tokio::test]
async fn cancels_during_summarisation_without_a_record() {
    let model = CodingModel {
        is_summary_stalled: true,
        ..CodingModel::default()
    };
    let harness = build_harness(&model, Arc::default());
    let mut agent = harness.spawn();
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    let watcher = model.requests.clone();
    tokio::spawn(async move {
        loop {
            tokio::task::yield_now().await;
            let is_summarising = watcher
                .lock()
                .unwrap()
                .iter()
                .any(|messages| check_is_summary_request(messages));
            if is_summarising {
                canceller.cancel();
                return;
            }
        }
    });
    let mut result = Ok(String::new());
    while result.is_ok() {
        result = agent.prompt_cancellable("work", &cancel).await;
    }
    assert!(matches!(result, Err(Error::Cancelled)));
    let has_record = agent
        .messages()
        .iter()
        .any(|message| message.as_compaction().is_some());
    assert!(!has_record);
    assert_valid_context(&agent.context());
}

fn build_harness(model: &CodingModel, store: Arc<MemoryStore>) -> harnessharness::Harness {
    let policy = CompactionPolicy::default()
        .trigger_tokens(TRIGGER_TOKENS)
        .keep_tokens(KEEP_TOKENS);
    HarnessHarness::new()
        .model(model.clone())
        .instructions("Be useful.")
        .tool(Echo)
        .compaction(policy)
        .session_store(SharedStore(store))
        .build()
        .unwrap()
}

fn assert_valid_context(messages: &[Message]) {
    assert_eq!(messages[0].role, Role::User, "{messages:#?}");
    let mut open_calls: HashSet<String> = HashSet::new();
    for message in messages {
        let results = tool_result_ids(message);
        assert_eq!(results, open_calls, "{messages:#?}");
        open_calls = message
            .tool_uses()
            .iter()
            .map(|tool_use| tool_use.id.clone())
            .collect();
        assert!(message.as_compaction().is_none());
    }
}

fn tool_result_ids(message: &Message) -> HashSet<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(tool_result) => Some(tool_result.tool_use_id.clone()),
            _ => None,
        })
        .collect()
}

fn check_is_summary_request(messages: &[Message]) -> bool {
    messages
        .last()
        .is_some_and(|message| message.text().starts_with(SUMMARY_REQUEST_START))
}

fn count_tool_calls_since_prompt(messages: &[Message]) -> usize {
    messages
        .iter()
        .rev()
        .take_while(|message| !check_is_prompt(message))
        .filter(|message| !message.tool_uses().is_empty())
        .count()
}

fn check_is_prompt(message: &Message) -> bool {
    message.role == Role::User && !message.text().is_empty()
}

fn measure(request: &CompletionRequest<'_>) -> Usage {
    let instructions_tokens = request.instructions.len() as u64 / 4;
    Usage {
        input_tokens: estimate(request.messages) + instructions_tokens,
        ..Usage::default()
    }
}

fn estimate(messages: &[Message]) -> u64 {
    serde_json::to_string(messages).unwrap().len() as u64 / 4
}

fn tool_call(id: &str, usage: Usage) -> Completion {
    let tool_use = ToolUse {
        id: id.to_string(),
        name: "echo".to_string(),
        input: json!({}),
    };
    Completion {
        content: vec![ContentBlock::ToolUse(tool_use)],
        stop_reason: StopReason::ToolUse,
        usage,
    }
}

fn reply(text: &str, usage: Usage) -> Completion {
    Completion {
        content: vec![ContentBlock::text(text)],
        stop_reason: StopReason::EndTurn,
        usage,
    }
}
