use harnessharness::HarnessHarness;
use harnessharness::model::provider::Anthropic;
use harnessharness::tool;
use harnessharness::tool::Tool;
use harnessharness::tool::ToolContext;
use harnessharness::tool::ToolDef;
use harnessharness::tool::ToolOutput;
use serde::Serialize;
use serde_json::json;

const AGENT_ID: &str = "agent-1";

#[derive(Serialize)]
struct Sum {
    total: i64,
}

/// Add two numbers.
///
/// Fails on overflow.
#[tool]
async fn add(
    /// First operand.
    left: i64,
    /// Second operand, defaults to 1.
    right: Option<i64>,
) -> Result<Sum, String> {
    let total = left.checked_add(right.unwrap_or(1)).ok_or("overflow")?;
    Ok(Sum { total })
}

/// Parse a number, failing with a standard library error.
#[tool]
async fn parse_number(text: String) -> Result<i64, std::num::ParseIntError> {
    text.parse::<i64>()
}

/// Report the calling agent.
#[tool]
fn whoami(context: &ToolContext) -> String {
    context.agent_id().to_string()
}

async fn call(tool: &ToolDef, input: serde_json::Value) -> Result<ToolOutput, String> {
    tool.call(input, &ToolContext::new(AGENT_ID)).await
}

#[test]
fn takes_name_and_description_from_function() {
    assert_eq!(add.name(), "add");
    assert_eq!(add.description(), "Add two numbers.\n\nFails on overflow.");
}

#[test]
fn generates_strict_input_schema() {
    assert_eq!(
        add.input_schema(),
        json!({
            "type": "object",
            "properties": {
                "left": {
                    "description": "First operand.",
                    "type": "integer",
                    "format": "int64"
                },
                "right": {
                    "description": "Second operand, defaults to 1.",
                    "type": ["integer", "null"],
                    "format": "int64"
                }
            },
            "required": ["left"],
            "additionalProperties": false
        })
    );
}

#[test]
fn generates_empty_schema_without_parameters() {
    assert_eq!(
        whoami.input_schema(),
        json!({ "type": "object", "additionalProperties": false })
    );
}

#[tokio::test]
async fn serializes_struct_output_as_json() {
    let output = call(&add, json!({ "left": 2, "right": 3 })).await.unwrap();
    assert_eq!(output, ToolOutput::Json(json!({ "total": 5 })));
}

#[tokio::test]
async fn defaults_optional_parameters() {
    let output = call(&add, json!({ "left": 2 })).await.unwrap();
    assert_eq!(output, ToolOutput::Json(json!({ "total": 3 })));
}

#[tokio::test]
async fn returns_tool_error() {
    let error = call(&add, json!({ "left": i64::MAX })).await.unwrap_err();
    assert_eq!(error, "overflow");
}

#[tokio::test]
async fn converts_display_errors() {
    let error = call(&parse_number, json!({ "text": "x" }))
        .await
        .unwrap_err();
    assert_eq!(error, "invalid digit found in string");
}

#[tokio::test]
async fn rejects_invalid_input() {
    let error = call(&add, json!({ "left": 1, "extra": true }))
        .await
        .unwrap_err();
    assert!(error.contains("unknown field `extra`"));
}

#[tokio::test]
async fn passes_context_and_raw_string_output() {
    let output = call(&whoami, json!({})).await.unwrap();
    assert_eq!(output, ToolOutput::text(AGENT_ID));
}

#[test]
fn registers_as_homogeneous_array() {
    let harness = HarnessHarness::new()
        .model(Anthropic::new("unused").api_key("unused"))
        .tools([add, parse_number, whoami])
        .build();
    assert!(harness.is_ok());
}
