use crate::agent::Agent;
use crate::error::Error;
use crate::model::Model;
use crate::retry::RetryPolicy;
use std::sync::Arc;

#[derive(Clone)]
pub struct Harness {
    pub(crate) model: Arc<dyn Model>,
    pub(crate) instructions: Arc<str>,
    pub(crate) retry: RetryPolicy,
}

#[derive(Default)]
pub struct HarnessBuilder {
    model: Option<Arc<dyn Model>>,
    instructions: Vec<String>,
    retry: RetryPolicy,
}

impl Harness {
    pub fn builder() -> HarnessBuilder {
        HarnessBuilder::default()
    }

    pub fn spawn(&self) -> Agent {
        Agent::new(self.clone())
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }
}

impl HarnessBuilder {
    pub fn model(mut self, model: impl Model + 'static) -> Self {
        self.model = Some(Arc::new(model));
        self
    }

    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions.push(instructions.into());
        self
    }

    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn build(self) -> Result<Harness, Error> {
        let model = self.model.ok_or(Error::MissingModel)?;
        let instructions = self.instructions.join("\n\n");
        Ok(Harness {
            model,
            instructions: Arc::from(instructions),
            retry: self.retry,
        })
    }
}
