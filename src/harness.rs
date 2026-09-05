use crate::agent::Agent;
use crate::error::Error;
use crate::model::Model;
use crate::retry::RetryPolicy;
use crate::tracer::TRACER_NAME;
use crate::tracer::Tracer;
use opentelemetry::global;
use opentelemetry::global::BoxedTracer;
use std::sync::Arc;

#[derive(Clone)]
pub struct Harness {
    pub(crate) model: Arc<dyn Model>,
    pub(crate) instructions: Arc<str>,
    pub(crate) retry: RetryPolicy,
    pub(crate) tracer: Arc<BoxedTracer>,
    tracer_provider: Option<Arc<dyn Tracer>>,
}

#[derive(Default)]
pub struct HarnessHarness {
    model: Option<Arc<dyn Model>>,
    instructions: Vec<String>,
    retry: RetryPolicy,
    tracer: Option<Arc<dyn Tracer>>,
}

impl Harness {
    pub fn builder() -> HarnessHarness {
        HarnessHarness::new()
    }

    pub fn spawn(&self) -> Agent {
        Agent::new(self.clone())
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        match &self.tracer_provider {
            Some(tracer_provider) => tracer_provider.shutdown(),
            None => Ok(()),
        }
    }
}

impl HarnessHarness {
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn tracer(mut self, tracer: impl Tracer + 'static) -> Self {
        self.tracer = Some(Arc::new(tracer));
        self
    }

    pub fn build(self) -> Result<Harness, Error> {
        let model = self.model.ok_or(Error::MissingModel)?;
        let instructions = self.instructions.join("\n\n");
        let tracer = match &self.tracer {
            Some(tracer_provider) => tracer_provider.tracer(),
            None => global::tracer(TRACER_NAME),
        };
        Ok(Harness {
            model,
            instructions: Arc::from(instructions),
            retry: self.retry,
            tracer: Arc::new(tracer),
            tracer_provider: self.tracer,
        })
    }
}
