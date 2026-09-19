mod terminal;

use crate::error::Error;
use crate::message::Message;
use std::future::Future;
pub use terminal::Terminal;

/// The user-facing side of `Harness::run_interactive`.
pub trait Frontend: Send {
    /// Wait for the next user input. `None` ends the session.
    fn read_input(&mut self) -> impl Future<Output = Result<Option<String>, Error>> + Send;

    /// Show the messages produced in reply to the last input.
    fn show_messages(&mut self, messages: &[Message]) -> Result<(), Error>;

    fn show_error(&mut self, error: &Error) -> Result<(), Error>;
}
