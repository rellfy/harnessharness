use async_trait::async_trait;
use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::model::provider::Anthropic;
use harnessharness::tool::Tool;
use harnessharness::tool::ToolError;
use harnessharness::tool::ToolOutput;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;

const ORDER_ID: &str = "A-1042";

#[derive(Deserialize)]
struct RefundOrderInput {
    order_id: String,
}

#[derive(Clone, Default)]
struct RefundOrder {
    refunded_order_ids: Arc<Mutex<Vec<String>>>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let refund_order = RefundOrder::default();
    let harness = HarnessHarness::new()
        .model(Anthropic::new("claude-haiku-4-5"))
        .instructions("you are an order support agent; always use the tools")
        .tool(refund_order.clone())
        .build()?;
    let mut agent = harness.spawn();
    let output = agent.prompt(format!("refund order {ORDER_ID}")).await?;
    println!("{output}");
    assert_eq!(refund_order.get_refunded_order_ids(), [ORDER_ID]);
    Ok(())
}

#[async_trait]
impl Tool for RefundOrder {
    fn name(&self) -> &str {
        "refund_order"
    }

    fn description(&self) -> &str {
        "Refund an order in full."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "order_id": {
                    "type": "string",
                    "description": "Order identifier, e.g. \"A-1042\"."
                }
            },
            "required": ["order_id"],
            "additionalProperties": false
        })
    }

    async fn call(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let input: RefundOrderInput = serde_json::from_value(input).map_err(ToolError::new)?;
        let mut refunded_order_ids = self.refunded_order_ids.lock().map_err(ToolError::new)?;
        refunded_order_ids.push(input.order_id.clone());
        Ok(ToolOutput::text(format!(
            "refunded order `{}`",
            input.order_id
        )))
    }
}

impl RefundOrder {
    fn get_refunded_order_ids(&self) -> Vec<String> {
        self.refunded_order_ids
            .lock()
            .map(|refunded_order_ids| refunded_order_ids.clone())
            .unwrap_or_default()
    }
}
