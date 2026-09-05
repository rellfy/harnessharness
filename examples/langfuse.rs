use harnessharness::Error;
use harnessharness::Harness;
use harnessharness::model::provider::Anthropic;
use harnessharness::tracer::provider::Langfuse;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let harness = Harness::builder()
        .model(Anthropic::new("claude-haiku-4-5"))
        .instructions("you are a 'hello world' greeter agent")
        .tracer(Langfuse::from_env()?)
        .build()?;
    let mut agent = harness.spawn();
    let output = agent.prompt("hello, say it back").await?;
    println!("{output}");
    harness.shutdown()?;
    Ok(())
}
