use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::model::provider::Anthropic;
use harnessharness::tool;
use harnessharness::tool::ToolContext;
use harnessharness::tool::ToolError;
use serde::Serialize;

const ORDER_ID: &str = "A-1042";
const CARRIER: &str = "Ferris Express";

#[derive(Serialize)]
struct OrderStatus {
    order_id: String,
    carrier: String,
    is_delivered: bool,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let harness = HarnessHarness::new()
        .model(Anthropic::new("claude-haiku-4-5"))
        .instructions(
            "you are an order support agent; \
            always use the tools, never guess order details",
        )
        .tools([get_order_status, add_order_note])
        .build()?;
    let mut agent = harness.spawn();
    let output = agent
        .prompt(format!(
            "which carrier has order {ORDER_ID}? \
            add a note that the customer called"
        ))
        .await?;
    println!("{output}");
    assert!(output.to_lowercase().contains(&CARRIER.to_lowercase()));
    Ok(())
}

/// Look up the shipping status of an order.
#[tool]
async fn get_order_status(
    /// Order identifier, e.g. "A-1042".
    order_id: String,
) -> Result<OrderStatus, ToolError> {
    match order_id.as_str() {
        ORDER_ID => Ok(OrderStatus {
            order_id,
            carrier: CARRIER.to_string(),
            is_delivered: false,
        }),
        _ => Err(ToolError::new(format!("order `{order_id}` not found"))),
    }
}

/// Attach a note to an order for the support team.
#[tool]
async fn add_order_note(
    /// Order identifier, e.g. "A-1042".
    order_id: String,
    /// Note text.
    note: String,
    /// Whether the note is hidden from the customer. Defaults to true.
    is_internal: Option<bool>,
    context: &ToolContext,
) -> Result<String, ToolError> {
    let visibility = match is_internal.unwrap_or(true) {
        true => "internal",
        false => "customer-visible",
    };
    let agent_id = context.agent_id();
    Ok(format!(
        "saved {visibility} note on order `{order_id}` by agent `{agent_id}`: {note}"
    ))
}
