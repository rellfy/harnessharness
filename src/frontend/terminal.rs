use crate::error::Error;
use crate::frontend::Frontend;
use crate::message::ContentBlock;
use crate::message::Message;
use crate::message::Role;
use std::env;
use std::io::Write;
use std::io::stdin;
use std::io::stdout;
use std::process::Command;
use std::process::Stdio;
use tokio::task::spawn_blocking;

const EXIT_COMMANDS: [&str; 2] = ["/exit", "/quit"];
const INPUT_PROMPT: &str = "> ";
const WORKING_STATUS: &str = "working…";
const TOOL_MARKER: &str = "● ";
const RULE: &str = "─";
const FOOTER_ROWS: usize = 3;
const DEFAULT_SIZE: Size = Size {
    rows: 24,
    columns: 80,
};
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const USER_COLORS: &str = "\x1b[48;5;238m\x1b[38;5;255m";
const CLEAR_LINE: &str = "\x1b[2K";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

/// A dependency-free terminal chat: message history above, input pinned to the bottom.
#[derive(Debug, Default)]
pub struct Terminal {
    entries: Vec<Entry>,
    is_started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    User,
    Assistant,
    Tool,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Size {
    rows: usize,
    columns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    AwaitingInput,
    Working,
}

impl Terminal {
    pub fn new() -> Self {
        Self::default()
    }

    fn draw(&mut self, status: Status) -> Result<(), Error> {
        let size = read_size();
        let mut output = stdout().lock();
        if !self.is_started {
            output.write_all("\n".repeat(size.rows).as_bytes())?;
            self.is_started = true;
        }
        output.write_all(render_frame(&self.entries, size, status).as_bytes())?;
        output.flush()?;
        Ok(())
    }

    fn push(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.entries.push(Entry {
            kind,
            text: text.into(),
        });
    }

    fn push_message(&mut self, message: &Message) {
        for block in &message.content {
            match (message.role, block) {
                (Role::Assistant, ContentBlock::Text { text }) => {
                    self.push(EntryKind::Assistant, text.trim())
                }
                (_, ContentBlock::ToolUse(tool_use)) => self.push(
                    EntryKind::Tool,
                    format!("{}{} {}", TOOL_MARKER, tool_use.name, tool_use.input),
                ),
                (Role::User, ContentBlock::Text { .. }) | (_, ContentBlock::ToolResult(_)) => {}
            }
        }
    }
}

impl Frontend for Terminal {
    async fn read_input(&mut self) -> Result<Option<String>, Error> {
        loop {
            self.draw(Status::AwaitingInput)?;
            let Some(line) = read_line().await? else {
                return Ok(None);
            };
            let input = line.trim();
            if EXIT_COMMANDS.contains(&input) {
                return Ok(None);
            }
            if input.is_empty() {
                continue;
            }
            self.push(EntryKind::User, input);
            self.draw(Status::Working)?;
            return Ok(Some(input.to_string()));
        }
    }

    fn show_messages(&mut self, messages: &[Message]) -> Result<(), Error> {
        for message in messages {
            self.push_message(message);
        }
        self.draw(Status::AwaitingInput)
    }

    fn show_error(&mut self, error: &Error) -> Result<(), Error> {
        self.push(EntryKind::Error, error.to_string());
        self.draw(Status::AwaitingInput)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if !self.is_started {
            return;
        }
        let size = read_size();
        let mut output = stdout().lock();
        let _ = writeln!(output, "{RESET}{SHOW_CURSOR}{}", move_to(size.rows, 1));
        let _ = output.flush();
    }
}

async fn read_line() -> Result<Option<String>, Error> {
    let line = spawn_blocking(|| {
        let mut line = String::new();
        let bytes_read = stdin().read_line(&mut line)?;
        Ok::<_, std::io::Error>((bytes_read, line))
    })
    .await
    .map_err(std::io::Error::other)??;
    match line {
        (0, _) => Ok(None),
        (_, line) => Ok(Some(line)),
    }
}

fn read_size() -> Size {
    read_stty_size()
        .or_else(read_env_size)
        .unwrap_or(DEFAULT_SIZE)
}

fn read_stty_size() -> Option<Size> {
    let output = Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_size(&String::from_utf8(output.stdout).ok()?)
}

fn read_env_size() -> Option<Size> {
    let rows = env::var("LINES").ok()?;
    let columns = env::var("COLUMNS").ok()?;
    parse_size(&format!("{rows} {columns}"))
}

fn parse_size(text: &str) -> Option<Size> {
    let mut parts = text.split_whitespace();
    let rows = parts.next()?.parse::<usize>().ok()?;
    let columns = parts.next()?.parse::<usize>().ok()?;
    match rows == 0 || columns == 0 {
        true => None,
        false => Some(Size { rows, columns }),
    }
}

fn render_frame(entries: &[Entry], size: Size, status: Status) -> String {
    let history_rows = size.rows.saturating_sub(FOOTER_ROWS);
    let lines = render_history(entries, size.columns);
    let visible = &lines[lines.len().saturating_sub(history_rows)..];
    let padding = history_rows - visible.len();
    let mut frame = String::from(HIDE_CURSOR);
    for row in 0..history_rows {
        let line = row
            .checked_sub(padding)
            .map(|index| visible[index].as_str());
        frame.push_str(&render_row(row + 1, line.unwrap_or_default()));
    }
    frame.push_str(&render_footer(size, status));
    frame
}

fn render_footer(size: Size, status: Status) -> String {
    let rule = format!("{DIM}{}{RESET}", RULE.repeat(size.columns));
    let input_row = size.rows.saturating_sub(1).max(1);
    let input = match status {
        Status::AwaitingInput => INPUT_PROMPT.to_string(),
        Status::Working => format!("{DIM}{INPUT_PROMPT}{WORKING_STATUS}{RESET}"),
    };
    let cursor_column = match status {
        Status::AwaitingInput => INPUT_PROMPT.chars().count() + 1,
        Status::Working => (INPUT_PROMPT.chars().count() + WORKING_STATUS.chars().count()) + 2,
    };
    [
        render_row(input_row.saturating_sub(1).max(1), &rule),
        render_row(input_row, &input),
        render_row(size.rows, &rule),
        move_to(input_row, cursor_column),
        SHOW_CURSOR.to_string(),
    ]
    .concat()
}

fn render_row(row: usize, content: &str) -> String {
    format!("{}{CLEAR_LINE}{content}", move_to(row, 1))
}

fn render_history(entries: &[Entry], columns: usize) -> Vec<String> {
    entries
        .iter()
        .flat_map(|entry| {
            let mut lines = render_entry(entry, columns);
            lines.push(String::new());
            lines
        })
        .collect()
}

fn render_entry(entry: &Entry, columns: usize) -> Vec<String> {
    match entry.kind {
        EntryKind::User => render_user_entry(&entry.text, columns),
        EntryKind::Assistant => wrap(&entry.text, columns),
        EntryKind::Tool => render_styled_entry(&entry.text, columns, DIM),
        EntryKind::Error => render_styled_entry(&entry.text, columns, RED),
    }
}

fn render_user_entry(text: &str, columns: usize) -> Vec<String> {
    let indent = INPUT_PROMPT.chars().count();
    let text_columns = columns.saturating_sub(indent + 1).max(1);
    wrap(text, text_columns)
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = match index {
                0 => INPUT_PROMPT.to_string(),
                _ => " ".repeat(indent),
            };
            let padding = " ".repeat(text_columns.saturating_sub(line.chars().count()) + 1);
            format!("{USER_COLORS}{prefix}{line}{padding}{RESET}")
        })
        .collect()
}

fn render_styled_entry(text: &str, columns: usize, style: &str) -> Vec<String> {
    wrap(text, columns)
        .iter()
        .map(|line| format!("{style}{line}{RESET}"))
        .collect()
}

fn wrap(text: &str, columns: usize) -> Vec<String> {
    text.lines()
        .flat_map(|line| wrap_line(&line.replace('\t', "    "), columns.max(1)))
        .collect()
}

fn wrap_line(line: &str, columns: usize) -> Vec<String> {
    let content = line.trim_start_matches(' ');
    let indent_length = (line.len() - content.len()).min(columns / 2);
    let indent = " ".repeat(indent_length);
    let content_columns = columns - indent_length;
    let mut lines = vec![String::new()];
    for word in content.split(' ') {
        for chunk in split_word(word, content_columns) {
            push_word(&mut lines, &chunk, content_columns);
        }
    }
    lines
        .iter()
        .map(|line| format!("{indent}{line}").trim_end().to_string())
        .collect()
}

fn push_word(lines: &mut Vec<String>, word: &str, columns: usize) {
    let current = lines.last_mut().expect("lines is never empty");
    let current_length = current.chars().count();
    let is_fitting = current_length + 1 + word.chars().count() <= columns;
    match (current_length, is_fitting) {
        (0, _) => current.push_str(word),
        (_, true) => {
            current.push(' ');
            current.push_str(word);
        }
        (_, false) => lines.push(word.to_string()),
    }
}

fn split_word(word: &str, columns: usize) -> Vec<String> {
    let characters = word.chars().collect::<Vec<char>>();
    match characters.is_empty() {
        true => vec![String::new()],
        false => characters
            .chunks(columns)
            .map(|chunk| chunk.iter().collect())
            .collect(),
    }
}

fn move_to(row: usize, column: usize) -> String {
    format!("\x1b[{row};{column}H")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolUse;
    use serde_json::json;

    const SIZE: Size = Size {
        rows: 8,
        columns: 20,
    };

    fn entry(kind: EntryKind, text: &str) -> Entry {
        Entry {
            kind,
            text: text.to_string(),
        }
    }

    #[test]
    fn wraps_on_word_boundaries() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    }

    #[test]
    fn splits_words_longer_than_the_width() {
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
    }

    #[test]
    fn keeps_blank_lines_and_indentation() {
        assert_eq!(wrap("a\n\n  b c d", 5), ["a", "", "  b c", "  d"]);
    }

    #[test]
    fn pads_user_lines_to_the_full_width() {
        let lines = render_user_entry("hello there big world", SIZE.columns);
        let expected = [
            format!("{USER_COLORS}> hello there big   {RESET}"),
            format!("{USER_COLORS}  world             {RESET}"),
        ];
        assert_eq!(lines, expected);
    }

    #[test]
    fn pins_the_input_between_rules_at_the_bottom() {
        let frame = render_frame(&[], SIZE, Status::AwaitingInput);
        let rule = format!("{DIM}{}{RESET}", RULE.repeat(SIZE.columns));
        assert!(frame.contains(&format!("\x1b[6;1H{CLEAR_LINE}{rule}")));
        assert!(frame.contains(&format!("\x1b[7;1H{CLEAR_LINE}{INPUT_PROMPT}")));
        assert!(frame.contains(&format!("\x1b[8;1H{CLEAR_LINE}{rule}")));
        assert!(frame.ends_with(&format!("\x1b[7;3H{SHOW_CURSOR}")));
    }

    #[test]
    fn shows_the_latest_history_directly_above_the_input() {
        let entries = [
            entry(EntryKind::Assistant, "first"),
            entry(EntryKind::Assistant, "second"),
            entry(EntryKind::Assistant, "third"),
        ];
        let frame = render_frame(&entries, SIZE, Status::AwaitingInput);
        assert!(!frame.contains("first"));
        assert!(frame.contains(&format!("\x1b[2;1H{CLEAR_LINE}second")));
        assert!(frame.contains(&format!("\x1b[4;1H{CLEAR_LINE}third")));
    }

    #[test]
    fn survives_a_tiny_terminal() {
        let size = Size {
            rows: 1,
            columns: 1,
        };
        let entries = [entry(EntryKind::User, "hello")];
        assert!(!render_frame(&entries, size, Status::Working).is_empty());
    }

    #[test]
    fn lists_tool_calls_and_assistant_text_but_not_results() {
        let mut terminal = Terminal::new();
        let tool_use = ContentBlock::ToolUse(ToolUse {
            id: "toolu_1".to_string(),
            name: "echo".to_string(),
            input: json!({ "text": "hi" }),
        });
        terminal.push_message(&Message::new(Role::Assistant, vec![tool_use]));
        terminal.push_message(&Message::tool_results(Vec::new()));
        terminal.push_message(&Message::assistant(" done \n"));
        let expected = [
            entry(EntryKind::Tool, "● echo {\"text\":\"hi\"}"),
            entry(EntryKind::Assistant, "done"),
        ];
        assert_eq!(terminal.entries, expected);
        terminal.is_started = false;
    }

    #[test]
    fn parses_stty_size() {
        assert_eq!(
            parse_size("40 120\n"),
            Some(Size {
                rows: 40,
                columns: 120
            })
        );
        assert_eq!(parse_size("0 0"), None);
        assert_eq!(parse_size(""), None);
    }
}
