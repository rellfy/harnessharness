use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::frontend::Terminal;
use harnessharness::model::provider::Anthropic;
use harnessharness::tool;
use std::time::SystemTime;
use std::time::SystemTimeError;
use std::time::UNIX_EPOCH;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let harness = HarnessHarness::new()
        .model(Anthropic::new("claude-haiku-4-5"))
        .instructions("you are a concise assistant in a terminal; reply in plain text")
        .tools([get_unix_time])
        .build()?;
    harness.run_interactive(Terminal::new()).await
}

/// Get the current time as seconds since the Unix epoch.
#[tool]
fn get_unix_time() -> Result<u64, SystemTimeError> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(elapsed.as_secs())
}
