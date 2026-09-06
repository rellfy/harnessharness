pub mod provider;

use crate::error::Error;
use crate::message::Message;
use async_trait::async_trait;

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn load(&self, session_id: &str) -> Result<Option<Vec<Message>>, Error>;

    async fn append(&self, session_id: &str, message: &Message) -> Result<(), Error>;
}
