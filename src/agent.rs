use crate::error::Error;
use crate::harness::Harness;
use crate::message::Message;
use crate::model::CompletionRequest;
use crate::retry::retry;
use crate::tracer::span::AgentSpan;
use uuid::Uuid;

pub struct Agent {
    id: String,
    harness: Harness,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(harness: Harness) -> Self {
        Self {
            id: Uuid::now_v7().to_string(),
            harness,
            messages: Vec::new(),
        }
    }

    pub async fn prompt(&mut self, input: impl Into<String>) -> Result<String, Error> {
        let input = input.into();
        let tracer = &self.harness.tracer;
        let model = &self.harness.model;
        let span = AgentSpan::start(tracer, "prompt", &self.id, &input);
        self.messages.push(Message::user(input));
        let request = CompletionRequest {
            instructions: &self.harness.instructions,
            messages: &self.messages,
        };
        let result = retry(&self.harness.retry, || async {
            let generation = span.generation(tracer, model.name(), &request);
            let completion_result = model.complete(request.clone()).await;
            generation.end(completion_result)
        })
        .await;
        let completion = span.end(result)?;
        self.messages.push(Message::assistant(&completion.text));
        Ok(completion.text)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}
