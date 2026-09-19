use serde::Serialize;
use serde_json::Error;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ToolOutput {
    Text(String),
    Json(Value),
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Serialize any value. Strings pass through as raw text.
    pub fn json(value: &impl Serialize) -> Result<Self, Error> {
        match serde_json::to_value(value)? {
            Value::String(text) => Ok(Self::Text(text)),
            value => Ok(Self::Json(value)),
        }
    }

    pub fn into_content(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Json(value) => value.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn passes_strings_through_as_text() {
        assert_eq!(ToolOutput::json(&"hi").unwrap(), ToolOutput::text("hi"));
    }

    #[test]
    fn keeps_tool_output_unchanged_when_serialized_again() {
        let outputs = [ToolOutput::text("hi"), ToolOutput::Json(json!({ "a": 1 }))];
        for output in outputs {
            assert_eq!(ToolOutput::json(&output).unwrap(), output);
        }
    }
}
