mod def;
mod output;

pub use def::ToolDef;
pub use def::ToolFuture;
pub use output::ToolOutput;

use serde::Serialize;
use serde_json::Value;
use std::fmt::Display;
use std::future::Future;

/// A tool the model can call.
/// Implement this for stateful tools, or use `#[tool]` on a function.
pub trait Tool: Send + Sync {
    type Output: Serialize;
    type Error: Display;

    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn input_schema(&self) -> Value;

    fn call(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}

pub(crate) trait DynTool: Send + Sync {
    fn name(&self) -> &str;

    fn spec(&self) -> ToolSpec;

    fn call<'a>(&'a self, input: Value, context: &'a ToolContext) -> ToolFuture<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContext {
    agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl<T: Tool> DynTool for T {
    fn name(&self) -> &str {
        Tool::name(self)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Tool::name(self).to_string(),
            description: Tool::description(self).to_string(),
            input_schema: Tool::input_schema(self),
        }
    }

    fn call<'a>(&'a self, input: Value, context: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            match Tool::call(self, input, context).await {
                Ok(output) => ToolOutput::json(&output).map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            }
        })
    }
}

impl ToolContext {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
        }
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}
