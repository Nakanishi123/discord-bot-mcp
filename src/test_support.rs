use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use serenity::http::HttpBuilder;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{config::Config, discord::Discord};

#[derive(Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub uri: String,
    pub body: Value,
}

type Replies = Arc<Mutex<VecDeque<(StatusCode, Value)>>>;
type Requests = Arc<Mutex<Vec<RecordedRequest>>>;

pub struct MockServer {
    pub url: String,
    pub requests: Requests,
    task: JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn respond(
    State((responses, requests)): State<(Replies, Requests)>,
    request: Request,
) -> Response {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let body = match to_bytes(request.into_body(), 65_536).await {
        Ok(bytes) if bytes.is_empty() => Value::Null,
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        },
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    requests
        .lock()
        .await
        .push(RecordedRequest { method, uri, body });
    let (status, response) = responses.lock().await.pop_front().unwrap_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"message":"unexpected request", "code":0}),
    ));
    (status, axum::Json(response)).into_response()
}

pub async fn mock_server(responses: Vec<(StatusCode, Value)>) -> anyhow::Result<MockServer> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let replies = Arc::new(Mutex::new(responses.into()));
    let app = Router::new()
        .fallback(any(respond))
        .with_state((replies, Arc::clone(&requests)));
    spawn(app, requests).await
}

async fn spawn(app: Router, requests: Requests) -> anyhow::Result<MockServer> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(MockServer {
        url,
        requests,
        task,
    })
}

pub async fn image_server(
    status: StatusCode,
    headers: HeaderMap,
    chunks: Vec<&'static [u8]>,
) -> anyhow::Result<MockServer> {
    let app = Router::new().fallback(any(move || {
        let headers = headers.clone();
        let chunks = chunks.clone();
        async move {
            let stream = tokio_util::io::ReaderStream::new(std::io::Cursor::new(chunks.concat()));
            (status, headers, Body::from_stream(stream))
        }
    }));
    spawn(app, Arc::new(Mutex::new(Vec::new()))).await
}

pub fn discord(server: &MockServer) -> anyhow::Result<Discord> {
    let client = reqwest::Client::builder().build()?;
    let http = HttpBuilder::new("test-token")
        .client(client)
        .proxy(&server.url)
        .ratelimiter_disabled(true)
        .build();
    Ok(Discord {
        http: Arc::new(http),
        channels: Arc::new(HashSet::from([123])),
    })
}

pub fn config() -> Config {
    Config {
        discord_token: "test-token".to_owned(),
        bearer_token: "a".repeat(32),
        channels: HashSet::from([123]),
        bind: ([127, 0, 0, 1], 0).into(),
        image_bytes: 1024,
        response_bytes: 100_000,
        image_concurrency: 2,
        image_timeout: std::time::Duration::from_secs(1),
        allowed_hosts: vec!["localhost".to_owned()],
        allowed_origins: vec![],
    }
}

pub fn message(id: &str) -> Value {
    json!({
        "id": id, "channel_id":"123", "guild_id":"456", "content":"test message",
        "author":{"id":"18446744073709551615", "username":"tester", "discriminator":"0", "avatar":null},
        "timestamp":"2026-10-04T00:00:00+00:00", "edited_timestamp":null,
        "tts":false, "mention_everyone":false, "mentions":[], "mention_roles":[],
        "attachments":[], "embeds":[], "pinned":false, "type":0
    })
}

pub async fn stalled_server() -> anyhow::Result<MockServer> {
    let app = Router::new().fallback(any(|| async { std::future::pending::<StatusCode>().await }));
    spawn(app, Arc::new(Mutex::new(Vec::new()))).await
}
