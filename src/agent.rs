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
        Self::restore(harness, Uuid::now_v7().to_string(), Vec::new())
    }

    pub fn restore(harness: Harness, id: String, messages: Vec<Message>) -> Self {
        Self {
            id,
            harness,
            messages,
        }
    }

    pub async fn prompt(&mut self, input: impl Into<String>) -> Result<String, Error> {
        let input = input.into();
        let tracer = self.harness.tracer.clone();
        let model = self.harness.model.clone();
        let span = AgentSpan::start(&tracer, "prompt", &self.id, &input);
        self.push_message(Message::user(input)).await?;
        let request = CompletionRequest {
            instructions: &self.harness.instructions,
            messages: &self.messages,
        };
        let result = retry(&self.harness.retry, || async {
            let generation = span.generation(&tracer, model.name(), &request);
            let completion_result = model.complete(request.clone()).await;
            generation.end(completion_result)
        })
        .await;
        let completion = span.end(result)?;
        self.push_message(Message::assistant(&completion.text))
            .await?;
        Ok(completion.text)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    async fn push_message(&mut self, message: Message) -> Result<(), Error> {
        if let Some(session_store) = &self.harness.session_store {
            session_store.append(&self.id, &message).await?;
        }
        self.messages.push(message);
        Ok(())
    }
}
