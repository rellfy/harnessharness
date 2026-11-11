use crate::message::Message;
use crate::message::Role;
use crate::tool::ToolSpec;
use std::borrow::Cow;

const DEFAULT_TRIGGER_TOKENS: u64 = 150_000;
const DEFAULT_KEEP_TOKENS: u64 = 20_000;
const BYTES_PER_TOKEN: u64 = 4;
pub(crate) const SUMMARY_REQUEST: &str = "\
Your context window is nearly full. \
Everything above this message will be replaced by the summary you write now, \
and the most recent messages will stay as they are. \
Write the summary for yourself, so you can continue the work without the original messages: \
the user's goals and requests (quote the latest request word for word), \
decisions made and why, the current state of the work, \
files, paths, commands and identifiers that still matter, \
errors met and how they were resolved, and what remains to do. \
Reply with the summary text only and do not call tools.";
const SUMMARY_PREFIX: &str = "\
This conversation was compacted to fit the context window. \
Summary of the earlier conversation:\n\n";

/// When and how an agent compacts its context.
/// Before each model call the agent estimates the context size; at `trigger_tokens` or more it
/// asks the model to summarise the oldest messages and keeps roughly the last `keep_tokens`
/// verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPolicy {
    pub trigger_tokens: u64,
    pub keep_tokens: u64,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            trigger_tokens: DEFAULT_TRIGGER_TOKENS,
            keep_tokens: DEFAULT_KEEP_TOKENS,
        }
    }
}

impl CompactionPolicy {
    pub fn trigger_tokens(mut self, trigger_tokens: u64) -> Self {
        self.trigger_tokens = trigger_tokens;
        self
    }

    pub fn keep_tokens(mut self, keep_tokens: u64) -> Self {
        self.keep_tokens = keep_tokens;
        self
    }
}

/// The messages a model sees for `history`: the latest compaction's summary, the messages it
/// kept, and everything after it.
pub(crate) fn build_context(history: &[Message]) -> Cow<'_, [Message]> {
    let Some(record_index) = history.iter().rposition(check_is_compaction) else {
        return Cow::Borrowed(history);
    };
    let Some(compaction) = history[record_index].as_compaction() else {
        return Cow::Borrowed(history);
    };
    let kept = collect_kept_messages(&history[..record_index], compaction.kept_message_count);
    let summary = Message::user(format!("{SUMMARY_PREFIX}{}", compaction.summary));
    let context = [summary]
        .into_iter()
        .chain(kept)
        .chain(history[record_index + 1..].iter().cloned())
        .collect();
    Cow::Owned(context)
}

/// Where to cut `context`: the summary replaces `context[..split]`.
/// The kept part starts at an assistant message, so it never holds a tool result whose call was
/// summarised away, and the summary (a user message) still alternates with it.
/// Keeps the longest such suffix within `keep_tokens`, else the shortest one.
pub(crate) fn find_split(context: &[Message], keep_tokens: u64) -> Option<usize> {
    let first_summarisable = match context.first().is_some_and(check_is_summary) {
        true => 2,
        false => 1,
    };
    let candidates: Vec<usize> = (first_summarisable..context.len())
        .filter(|index| context[*index].role == Role::Assistant)
        .collect();
    let within_budget = candidates
        .iter()
        .find(|index| estimate_tokens(&context[**index..]) <= keep_tokens);
    within_budget.or(candidates.last()).copied()
}

pub(crate) fn estimate_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum()
}

pub(crate) fn estimate_prompt_tokens(instructions: &str, tools: &[ToolSpec]) -> u64 {
    let tools_bytes = serde_json::to_string(tools).map_or(0, |json| json.len());
    (instructions.len() + tools_bytes) as u64 / BYTES_PER_TOKEN
}

fn estimate_message_tokens(message: &Message) -> u64 {
    let bytes = serde_json::to_string(message).map_or(0, |json| json.len());
    bytes as u64 / BYTES_PER_TOKEN
}

fn collect_kept_messages(before_record: &[Message], kept_message_count: usize) -> Vec<Message> {
    let mut kept: Vec<Message> = before_record
        .iter()
        .rev()
        .filter(|message| !check_is_compaction(message))
        .take(kept_message_count)
        .cloned()
        .collect();
    kept.reverse();
    kept
}

fn check_is_compaction(message: &Message) -> bool {
    message.as_compaction().is_some()
}

fn check_is_summary(message: &Message) -> bool {
    message.role == Role::User && message.text().starts_with(SUMMARY_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Compaction;
    use crate::message::ContentBlock;
    use crate::message::ToolResult;
    use crate::message::ToolUse;
    use serde_json::json;

    fn tool_call(id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUse {
                id: id.to_string(),
                name: "echo".to_string(),
                input: json!({}),
            })],
        )
    }

    fn tool_result(id: &str, content: &str) -> Message {
        Message::tool_results(vec![ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: false,
        }])
    }

    fn record(summary: &str, kept_message_count: usize) -> Message {
        Message::compaction(Compaction {
            summary: summary.to_string(),
            kept_message_count,
        })
    }

    fn summary(text: &str) -> Message {
        Message::user(format!("{SUMMARY_PREFIX}{text}"))
    }

    #[test]
    fn passes_uncompacted_history_through() {
        let history = [Message::user("hi"), Message::assistant("hello")];
        assert!(matches!(build_context(&history), Cow::Borrowed(_)));
    }

    #[test]
    fn replaces_everything_before_the_kept_messages_with_the_summary() {
        let history = [
            Message::user("a"),
            tool_call("1"),
            tool_result("1", "x"),
            Message::assistant("b"),
            record("did a", 1),
            Message::user("c"),
        ];
        let expected = [
            summary("did a"),
            Message::assistant("b"),
            Message::user("c"),
        ];
        assert_eq!(build_context(&history).as_ref(), expected);
    }

    #[test]
    fn skips_earlier_records_when_counting_kept_messages() {
        let history = [
            Message::user("a"),
            Message::assistant("b"),
            record("first", 1),
            Message::user("c"),
            Message::assistant("d"),
            record("second", 3),
        ];
        let expected = [
            summary("second"),
            Message::assistant("b"),
            Message::user("c"),
            Message::assistant("d"),
        ];
        assert_eq!(build_context(&history).as_ref(), expected);
    }

    #[test]
    fn splits_at_an_assistant_message_within_the_budget() {
        let context = [
            Message::user("a"),
            tool_call("1"),
            tool_result("1", &"x".repeat(4_000)),
            tool_call("2"),
            tool_result("2", "y"),
            Message::assistant("done"),
        ];
        assert_eq!(find_split(&context, 100), Some(3));
        assert_eq!(find_split(&context, 10_000), Some(1));
    }

    #[test]
    fn keeps_the_last_exchange_when_nothing_fits() {
        let context = [
            Message::user("a"),
            tool_call("1"),
            tool_result("1", &"x".repeat(4_000)),
        ];
        assert_eq!(find_split(&context, 10), Some(1));
    }

    #[test]
    fn never_summarises_only_the_previous_summary() {
        let context = [summary("old"), tool_call("1"), tool_result("1", "x")];
        assert_eq!(find_split(&context, 0), None);
        assert_eq!(find_split(&[Message::user("only")], 0), None);
    }
}
