//! Legacy HTTP+SSE transport: one GET event stream announces a POST endpoint,
//! and every JSON-RPC message travels one way or the other.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Sink, Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use reqwest::Url;
use rmcp::service::{RoleClient, RxJsonRpcMessage, TxJsonRpcMessage};
use sse_stream::SseStream;
use tokio::sync::mpsc;

use super::{connect_failed, header_pairs, Server};
use crate::ToolResult;

/// Start the event stream and hand rmcp a sink and a stream over it.
pub(super) async fn connect(server: &Server, url: &str) -> ToolResult<SseTransport> {
    let target = Url::parse(url).map_err(|error| connect_failed(server, url, error))?;
    let headers = header_map(server)?;
    let client = reqwest::Client::new();
    let response = client
        .get(target.clone())
        .headers(headers.clone())
        .send()
        .await
        .map_err(|error| connect_failed(server, url, error))?;
    if !response.status().is_success() {
        return Err(connect_failed(
            server,
            url,
            format!("HTTP status {}", response.status()),
        ));
    }

    let (outbound, mut outgoing) = mpsc::unbounded_channel::<String>();
    let (incoming, inbound) = mpsc::unbounded_channel::<RxJsonRpcMessage<RoleClient>>();
    tokio::spawn(async move {
        let mut events = SseStream::from_bytes_stream(response.bytes_stream());
        let mut endpoint: Option<Url> = None;
        // rmcp sends `initialize` before the endpoint event can arrive.
        let mut pending = Vec::new();
        loop {
            tokio::select! {
                message = outgoing.recv() => {
                    let Some(message) = message else { return };
                    match &endpoint {
                        Some(destination) => {
                            if !post_message(&client, destination, &headers, message).await {
                                return;
                            }
                        }
                        None => pending.push(message),
                    }
                }
                event = events.next() => {
                    match event {
                        Some(Ok(event)) if event.event.as_deref() == Some("endpoint") => {
                            let Some(data) = event.data else { return };
                            let Ok(post) = target.join(&data) else { return };
                            endpoint = Some(post.clone());
                            for message in pending.drain(..) {
                                if !post_message(&client, &post, &headers, message).await {
                                    return;
                                }
                            }
                        }
                        Some(Ok(event))
                            if event.is_message() || event.event.as_deref() == Some("message") =>
                        {
                            let Some(data) = event.data else { continue };
                            let Ok(message) = serde_json::from_str(&data) else { return };
                            if incoming.send(message).is_err() {
                                return;
                            }
                        }
                        // A closed stream ends this task; rmcp reports "Transport closed".
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => return,
                    }
                }
            }
        }
    });
    Ok(SseTransport { outbound, inbound })
}

fn header_map(server: &Server) -> ToolResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in header_pairs(server)? {
        headers.insert(name, value);
    }
    headers
        .entry(ACCEPT)
        .or_insert(HeaderValue::from_static("text/event-stream"));
    Ok(headers)
}

async fn post_message(
    client: &reqwest::Client,
    url: &Url,
    headers: &HeaderMap,
    body: String,
) -> bool {
    match client
        .post(url.clone())
        .timeout(super::CALL_TIMEOUT)
        .headers(headers.clone())
        .body(body)
        .send()
        .await
    {
        Ok(reply) if reply.status().is_success() => {
            let _ = reply.bytes().await;
            true
        }
        _ => false,
    }
}

/// rmcp's `(Sink, Stream)` view of the event stream.
pub(super) struct SseTransport {
    outbound: mpsc::UnboundedSender<String>,
    inbound: mpsc::UnboundedReceiver<RxJsonRpcMessage<RoleClient>>,
}

impl Stream for SseTransport {
    type Item = RxJsonRpcMessage<RoleClient>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inbound.poll_recv(cx)
    }
}

impl Sink<TxJsonRpcMessage<RoleClient>> for SseTransport {
    type Error = Closed;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.get_mut().outbound.is_closed() {
            Poll::Ready(Err(Closed))
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(
        self: Pin<&mut Self>,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> Result<(), Self::Error> {
        let message = serde_json::to_string(&item).map_err(|_| Closed)?;
        self.get_mut().outbound.send(message).map_err(|_| Closed)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// The stream behind this sink is gone.
#[derive(Debug)]
pub(super) struct Closed;

impl std::fmt::Display for Closed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Transport closed")
    }
}

impl std::error::Error for Closed {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::convert::Infallible;
    use std::future::IntoFuture;
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::sse::{Event, Sse};
    use axum::routing::{get, post};
    use axum::Router;
    use rmcp::model::CallToolRequestParams;
    use serde_json::{json, Value};
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone, Default)]
    struct Hub {
        client: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Hub {
        fn record(&self, method: &str, headers: &HeaderMap) {
            let field = |name: &str| {
                headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("-")
                    .to_string()
            };
            let api_key = field("x-api-key");
            let accept = field("accept");
            self.seen
                .lock()
                .unwrap()
                .push(format!("{method} {api_key} {accept}"));
        }
    }

    async fn stream(
        State(hub): State<Hub>,
        headers: HeaderMap,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        hub.record("GET", &headers);
        let (client, messages) = mpsc::unbounded_channel::<String>();
        *hub.client.lock().unwrap() = Some(client);
        let endpoint = futures::stream::once(async {
            Ok(Event::default().event("endpoint").data("/messages"))
        });
        let messages = futures::stream::unfold(messages, |mut messages| async move {
            let message = messages.recv().await?;
            Some((
                Ok(Event::default().event("message").data(message)),
                messages,
            ))
        });
        Sse::new(endpoint.chain(messages))
    }

    async fn messages(State(hub): State<Hub>, headers: HeaderMap, body: String) -> StatusCode {
        hub.record("POST", &headers);
        let Ok(request) = serde_json::from_str::<Value>(&body) else {
            return StatusCode::BAD_REQUEST;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let result = match request["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "legacy", "version": "0"},
            }),
            Some("tools/list") => json!({
                "tools": [{"name": "echo", "description": "Echo back.", "inputSchema": {"type": "object"}}],
            }),
            Some("tools/call") => json!({
                "content": [{"type": "text", "text": "pong"}],
            }),
            _ => return StatusCode::ACCEPTED,
        };
        let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
        if let Some(client) = hub.client.lock().unwrap().as_ref() {
            let _ = client.send(reply.to_string());
        }
        StatusCode::ACCEPTED
    }

    async fn shutdown(State(hub): State<Hub>) -> StatusCode {
        hub.client.lock().unwrap().take();
        StatusCode::ACCEPTED
    }

    #[tokio::test]
    async fn a_legacy_sse_server_lists_and_answers_with_its_headers() {
        let hub = Hub::default();
        let app = Router::new()
            .route("/sse", get(stream))
            .route("/messages", post(messages))
            .route("/shutdown", get(shutdown))
            .with_state(hub.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let url = format!("http://{address}/sse");
        let entry: super::super::Entry = serde_json::from_value(json!({
            "command": null,
            "url": url,
            "transport": "sse",
            "headers": {"x-api-key": "secret"},
        }))
        .unwrap();
        let server = super::super::Server {
            name: "legacy".to_string(),
            entry,
            error: None,
            failed: Mutex::new(None),
        };

        let legacy = super::super::connect_sse(&server, &url).await.unwrap();
        let names: Vec<&str> = legacy.tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(names, ["echo"]);
        let answer = legacy
            .service
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .unwrap();
        let text = answer
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|block| block.text.as_str());
        assert_eq!(text, Some("pong"));

        let seen = hub.seen.lock().unwrap().clone();
        assert!(
            seen.iter()
                .any(|line| line == "GET secret text/event-stream"),
            "{seen:?}"
        );
        assert!(
            seen.iter()
                .any(|line| line == "POST secret text/event-stream"),
            "{seen:?}"
        );

        reqwest::get(format!("http://{address}/shutdown"))
            .await
            .unwrap();
        let error = legacy
            .service
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Transport closed"), "{error}");
    }
}
