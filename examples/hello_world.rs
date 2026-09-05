use harnessharness::Error;
use harnessharness::Harness;
use harnessharness::model::provider::Anthropic;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let harness = Harness::builder()
        .model(Anthropic::new("claude-haiku-4-5"))
        .instructions("you are a 'hello world' greeter agent")
        .build()?;
    let mut agent = harness.spawn();
    let output = agent.prompt("hello, say it back").await?;
    println!("{output}");
    assert!(output.to_lowercase().contains("hello"));
    Ok(())
}
