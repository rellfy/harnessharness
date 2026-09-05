use crate::error::Error;
use crate::harness::Harness;
use crate::message::Message;
use crate::model::CompletionRequest;

pub struct Agent {
    harness: Harness,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(harness: Harness) -> Self {
        Self {
            harness,
            messages: Vec::new(),
        }
    }

    pub async fn prompt(&mut self, input: impl Into<String>) -> Result<String, Error> {
        self.messages.push(Message::user(input));
        let request = CompletionRequest {
            instructions: &self.harness.instructions,
            messages: &self.messages,
        };
        let completion = self.harness.model.complete(request).await?;
        self.messages.push(Message::assistant(&completion.text));
        Ok(completion.text)
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}
