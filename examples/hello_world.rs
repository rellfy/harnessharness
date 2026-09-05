fn main() {
    let harness = Harness::builder()
        .model(Anthropic::new("claude-haiku-4.5"))
        .instructions("you are a 'hello world' greeter agent")
        .build()?;
    let agent = harness.spawn();
    let output = agent.prompt("hello, say it back");
    println!("{output}");
    assert!(output.to_lowercase().contains("hello"));
}
