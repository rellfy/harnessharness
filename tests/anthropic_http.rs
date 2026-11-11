use harnessharness::CancelToken;
use harnessharness::Error;
use harnessharness::HarnessHarness;
use harnessharness::RetryPolicy;
use harnessharness::message::Message;
use harnessharness::model::StreamEvent;
use harnessharness::model::provider::Anthropic;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::sync::oneshot::Sender;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio::time::timeout;

const STREAM_HEADERS: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
connection: close\r\n\r\n";
const JSON_HEADERS: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
connection: close\r\n\r\n";
const MESSAGE_START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\
\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n";
const TEXT_START: &str = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\
\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";
const HELLO_DELTA: &str = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\
\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n";
const WORLD_DELTA: &str = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\
\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo!\"}}\n\n";
const STREAM_END: &str = "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\
\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":\
{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\nevent: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";
const JSON_REPLY: &str = "{\"content\":[{\"type\":\"text\",\"text\":\"Hello!\"}],\
\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Close,
    HoldOpen,
}

struct Server {
    base_url: String,
    request_body: oneshot::Receiver<Value>,
    task: JoinHandle<bool>,
}

#[tokio::test]
async fn streams_a_reply_over_http() {
    let server = Server::start(
        vec![
            STREAM_HEADERS,
            MESSAGE_START,
            TEXT_START,
            HELLO_DELTA,
            WORLD_DELTA,
            STREAM_END,
        ],
        Ending::HoldOpen,
    )
    .await;
    let deltas: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = deltas.clone();
    let harness = HarnessHarness::new()
        .model(provider(&server.base_url))
        .instructions("Be brief.")
        .on_stream(move |_, event| {
            if let StreamEvent::TextDelta { text, .. } = event {
                seen.lock().unwrap().push(text.clone());
            }
        })
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    assert_eq!(agent.prompt("hi").await.unwrap(), "Hello!");
    assert_eq!(*deltas.lock().unwrap(), ["Hel", "lo!"]);
    assert_eq!(agent.messages()[1], Message::assistant("Hello!"));
    let body = server.request_body.await.unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["cache_control"]["type"], "ephemeral");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
}

#[tokio::test]
async fn keeps_the_non_streaming_request_without_a_listener() {
    let server = Server::start(vec![JSON_HEADERS, JSON_REPLY], Ending::Close).await;
    let harness = HarnessHarness::new()
        .model(provider(&server.base_url))
        .build()
        .unwrap();
    assert_eq!(harness.spawn().prompt("hi").await.unwrap(), "Hello!");
    let body = server.request_body.await.unwrap();
    assert!(body.get("stream").is_none());
}

#[tokio::test]
async fn closes_the_connection_when_cancelled_mid_stream() {
    let server = Server::start(
        vec![STREAM_HEADERS, MESSAGE_START, TEXT_START, HELLO_DELTA],
        Ending::HoldOpen,
    )
    .await;
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    let harness = HarnessHarness::new()
        .model(provider(&server.base_url))
        .retry(RetryPolicy::none())
        .on_stream(move |_, event| {
            if matches!(event, StreamEvent::TextDelta { .. }) {
                canceller.cancel();
            }
        })
        .build()
        .unwrap();
    let mut agent = harness.spawn();
    let result = agent.prompt_cancellable("hi", &cancel).await;
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(agent.messages(), [Message::user("hi")]);
    let is_closed = timeout(Duration::from_secs(5), server.task).await;
    assert!(is_closed.unwrap().unwrap());
}

impl Server {
    async fn start(chunks: Vec<&'static str>, ending: Ending) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (body_sender, request_body) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            serve(socket, chunks, ending, body_sender).await
        });
        Self {
            base_url,
            request_body,
            task,
        }
    }
}

async fn serve(
    mut socket: TcpStream,
    chunks: Vec<&'static str>,
    ending: Ending,
    body_sender: Sender<Value>,
) -> bool {
    let body = read_request_body(&mut socket).await;
    let _ = body_sender.send(body);
    for chunk in chunks {
        socket.write_all(chunk.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        sleep(Duration::from_millis(5)).await;
    }
    match ending {
        Ending::Close => socket.shutdown().await.is_ok(),
        Ending::HoldOpen => wait_for_close(&mut socket).await,
    }
}

async fn read_request_body(socket: &mut TcpStream) -> Value {
    let mut received = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let read = socket.read(&mut buffer).await.unwrap();
        received.extend_from_slice(&buffer[..read]);
        if let Some(body) = split_complete_body(&received) {
            return serde_json::from_slice(body).unwrap();
        }
    }
}

fn split_complete_body(received: &[u8]) -> Option<&[u8]> {
    let header_end = received
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?
        + 4;
    let headers = String::from_utf8_lossy(&received[..header_end]).to_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))?
        .trim()
        .parse::<usize>()
        .ok()?;
    let body = &received[header_end..];
    match body.len() >= content_length {
        true => Some(&body[..content_length]),
        false => None,
    }
}

async fn wait_for_close(socket: &mut TcpStream) -> bool {
    let mut buffer = [0; 64];
    loop {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return true,
            Ok(_) => {}
        }
    }
}

fn provider(base_url: &str) -> Anthropic {
    Anthropic::new("model")
        .api_key("test-key")
        .base_url(base_url)
}
