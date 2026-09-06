use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::model::provider::Anthropic;
use harnessharness::session::provider::Sqlite;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let harness = HarnessHarness::new()
        .model(Anthropic::new("claude-haiku-4-5"))
        .instructions("you are a concise assistant")
        .session_store(Sqlite::connect("sessions.db").await?)
        .build()?;
    let session_id = {
        let mut agent = harness.spawn();
        println!("{}", agent.prompt("my name is Ferris").await?);
        agent.id().to_string()
    };
    let mut resumed_agent = harness.resume_session(session_id).await?;
    let output = resumed_agent.prompt("what is my name?").await?;
    println!("{output}");
    assert!(output.to_lowercase().contains("ferris"));
    Ok(())
}
