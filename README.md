# harnessharness
We heard you like a harness, so we made a harness for your harness.

`harnessharness` lets you Easily declare a harness using the builder pattern
and spawn it into an agent that is ready to be deployed and prompted.

## Example
```rust
let harness = HarnessHarness::new()
    .model(OpenRouter::new("anthropic/claude-sonnet-4.5"))
    .instructions(include_str!("instructions.md"))
    .tools([read_file, write_file, bash, grep])
    .subagent("explore", HarnessHarness::new().tools([read_file, grep]).read_only())
    .policy(Policy::new()
        .allow(read_file)
        .allow(grep)
        .ask(write_file)
        .ask(bash.matching("rm|git push")))
    .approver(TerminalApprover)
    .compaction(Compaction::summarize_at(120_000))
    .hook(post_tool(|call, result| async move {
        if call.tool == "write_file" && call.path().ends_with(".rs") {
            Command::new("cargo").args(["fmt", "--", call.path()]).status().await?;
        }
        Ok(result)
    }))
    .retry(RetryPolicy::default().max_attempts(5))
    .session_store(Sqlite::connect("sessions.db").await?)
    .tracer(Langfuse::from_env()?)
    .build()?;

harness.run_interactive(Terminal).await?;
```
