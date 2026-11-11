use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use strum::Display;
use strum::EnumString;
use strum::IntoStaticStr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, IntoStaticStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    Compaction(Compaction),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Marks where an agent compacted its context.
/// Models never see this block: from here on they get `summary` in place of everything before
/// the last `kept_message_count` messages preceding it.
/// Earlier messages stay in the history, so a transcript still shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compaction {
    pub summary: String,
    pub kept_message_count: usize,
}

impl Message {
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::text(text)])
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentBlock::text(text)])
    }

    pub fn tool_results(tool_results: Vec<ToolResult>) -> Self {
        let content = tool_results
            .into_iter()
            .map(ContentBlock::ToolResult)
            .collect();
        Self::new(Role::User, content)
    }

    pub fn compaction(compaction: Compaction) -> Self {
        Self::new(Role::User, vec![ContentBlock::Compaction(compaction)])
    }

    pub fn as_compaction(&self) -> Option<&Compaction> {
        self.content.iter().find_map(|block| match block {
            ContentBlock::Compaction(compaction) => Some(compaction),
            ContentBlock::Text { .. } | ContentBlock::ToolUse(_) | ContentBlock::ToolResult(_) => {
                None
            }
        })
    }

    pub fn text(&self) -> String {
        join_text(&self.content)
    }

    pub fn tool_uses(&self) -> Vec<&ToolUse> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => Some(tool_use),
                ContentBlock::Text { .. }
                | ContentBlock::ToolResult(_)
                | ContentBlock::Compaction(_) => None,
            })
            .collect()
    }
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

pub(crate) fn join_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolUse(_)
            | ContentBlock::ToolResult(_)
            | ContentBlock::Compaction(_) => None,
        })
        .collect::<Vec<&str>>()
        .join("")
}
