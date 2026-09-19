use crate::agent::Agent;
use crate::error::Error;
use crate::model::Model;
use crate::retry::RetryPolicy;
use crate::session::SessionStore;
use crate::tool::DynTool;
use crate::tool::Tool;
use crate::tool::ToolSpec;
use crate::tracer::TRACER_NAME;
use crate::tracer::Tracer;
use opentelemetry::global;
use opentelemetry::global::BoxedTracer;
use std::collections::HashSet;
use std::sync::Arc;

const DEFAULT_MAX_ITERATIONS: u32 = 50;

#[derive(Clone)]
pub struct Harness {
    pub(crate) model: Arc<dyn Model>,
    pub(crate) instructions: Arc<str>,
    pub(crate) retry: RetryPolicy,
    pub(crate) tracer: Arc<BoxedTracer>,
    pub(crate) session_store: Option<Arc<dyn SessionStore>>,
    pub(crate) tools: Arc<[Arc<dyn DynTool>]>,
    pub(crate) tool_specs: Arc<[ToolSpec]>,
    pub(crate) max_iterations: u32,
    tracer_provider: Option<Arc<dyn Tracer>>,
}

pub struct HarnessHarness {
    model: Option<Arc<dyn Model>>,
    instructions: Vec<String>,
    retry: RetryPolicy,
    tracer: Option<Arc<dyn Tracer>>,
    session_store: Option<Arc<dyn SessionStore>>,
    tools: Vec<Arc<dyn DynTool>>,
    max_iterations: u32,
}

impl Harness {
    pub fn builder() -> HarnessHarness {
        HarnessHarness::new()
    }

    /// Spawn a new agent.
    pub fn spawn(&self) -> Agent {
        Agent::new(self.clone())
    }

    /// Spawn an agent resuming from an existing session ID.
    pub async fn resume_session(&self, session_id: impl Into<String>) -> Result<Agent, Error> {
        let session_id = session_id.into();
        let session_store = self
            .session_store
            .as_ref()
            .ok_or(Error::MissingSessionStore)?;
        let messages = session_store
            .load(&session_id)
            .await?
            .ok_or_else(|| Error::SessionNotFound(session_id.clone()))?;
        Ok(Agent::restore(self.clone(), session_id, messages))
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

    pub(crate) fn find_tool(&self, name: &str) -> Option<&Arc<dyn DynTool>> {
        self.tools.iter().find(|tool| tool.name() == name)
    }
}

impl Default for HarnessHarness {
    fn default() -> Self {
        Self {
            model: None,
            instructions: Vec::new(),
            retry: RetryPolicy::default(),
            tracer: None,
            session_store: None,
            tools: Vec::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
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

    pub fn session_store(mut self, session_store: impl SessionStore + 'static) -> Self {
        self.session_store = Some(Arc::new(session_store));
        self
    }

    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.push(Arc::new(tool));
        self
    }

    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Tool + 'static>) -> Self {
        let tools = tools
            .into_iter()
            .map(|tool| Arc::new(tool) as Arc<dyn DynTool>);
        self.tools.extend(tools);
        self
    }

    /// Maximum model calls per prompt, guarding against endless tool loops.
    pub fn max_iterations(mut self, max_iterations: u32) -> Self {
        self.max_iterations = max_iterations.max(1);
        self
    }

    pub fn build(self) -> Result<Harness, Error> {
        let model = self.model.ok_or(Error::MissingModel)?;
        check_tool_names_are_unique(&self.tools)?;
        let tool_specs = self.tools.iter().map(|tool| tool.spec()).collect();
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
            session_store: self.session_store,
            tools: Arc::from(self.tools),
            tool_specs,
            max_iterations: self.max_iterations,
            tracer_provider: self.tracer,
        })
    }
}

fn check_tool_names_are_unique(tools: &[Arc<dyn DynTool>]) -> Result<(), Error> {
    let mut names = HashSet::new();
    let duplicate = tools.iter().find(|tool| !names.insert(tool.name()));
    match duplicate {
        Some(tool) => Err(Error::DuplicateTool(tool.name().to_string())),
        None => Ok(()),
    }
}
