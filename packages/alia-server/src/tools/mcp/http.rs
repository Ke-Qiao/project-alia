//! Bounded HTTP body handling around the official MCP transport state machine.
use axum::http::{HeaderName, HeaderValue};
use futures::{StreamExt, stream::BoxStream};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, ServerJsonRpcMessage},
    transport::streamable_http_client::{
        SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    },
};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc, time::Duration};

use super::MAX_MESSAGE_BYTES;

type HttpError = StreamableHttpError<std::io::Error>;
#[derive(Clone)]
pub(super) struct BoundedHttp {
    client: reqwest::Client,
    endpoint: Arc<str>,
}
impl BoundedHttp {
    pub fn new(endpoint: &str) -> anyhow::Result<Self> {
        let url = reqwest::Url::parse(endpoint)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "MCP endpoint must be an HTTP(S) service URL without credentials/fragment"
        );
        Ok(Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()?,
        })
    }
    fn request(
        &self,
        method: reqwest::Method,
        uri: &str,
        session: Option<&str>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<reqwest::RequestBuilder, HttpError> {
        if uri != self.endpoint.as_ref() {
            return Err(failure("MCP endpoint changed"));
        }
        let mut request = self
            .client
            .request(method, uri)
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        for (name, value) in headers {
            if matches!(
                name.as_str(),
                "authorization" | "host" | "cookie" | "proxy-authorization"
            ) {
                return Err(failure("reserved MCP request header"));
            }
            request = request.header(name, value);
        }
        Ok(request)
    }
}
fn failure(message: &str) -> HttpError {
    HttpError::Io(std::io::Error::other(message.to_owned()))
}

#[derive(Clone, Copy)]
enum EventMode {
    Response,
    Subscription,
}
impl EventMode {
    fn subscription(self) -> bool {
        matches!(self, Self::Subscription)
    }
}
// The SSE library buffers unfinished lines and accumulates data until a blank
// line. Track those exact CR/LF boundaries before handing bytes to its parser.
// Window resets never reset an unfinished frame's allocation budget.
struct EventBudget {
    mode: EventMode,
    window: tokio::time::Instant,
    bytes: usize,
    frame: usize,
    line_has_data: bool,
    skip_lf: bool,
}
impl EventBudget {
    fn new(mode: EventMode) -> Self {
        Self {
            mode,
            window: tokio::time::Instant::now(),
            bytes: 0,
            frame: 0,
            line_has_data: false,
            skip_lf: false,
        }
    }
    fn consume(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if self.mode.subscription() && self.window.elapsed() >= Duration::from_secs(60) {
            self.window = tokio::time::Instant::now();
            self.bytes = 0;
        }
        if chunk.len() > MAX_MESSAGE_BYTES.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("MCP SSE byte budget"));
        }
        self.bytes += chunk.len();
        for &byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.frame += 1;
            if self.frame > MAX_MESSAGE_BYTES {
                return Err(std::io::Error::other("MCP SSE frame byte budget"));
            }
            if matches!(byte, b'\r' | b'\n') {
                if !self.line_has_data {
                    self.frame = 0;
                }
                self.line_has_data = false;
                self.skip_lf = byte == b'\r';
            } else {
                self.line_has_data = true;
            }
        }
        Ok(())
    }
}
fn bounded_events(
    response: reqwest::Response,
    mode: EventMode,
) -> BoxStream<'static, Result<Sse, SseError>> {
    let stream = async_stream::try_stream! {
        let mut source = response.bytes_stream();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut budget = EventBudget::new(mode);
        loop {
            let item = if mode.subscription() {
                // An idle notification connection owns no growing buffer. It is
                // closed by the original transport owner, not a response timer.
                source.next().await
            } else {
                tokio::time::timeout_at(deadline, source.next()).await.map_err(|_| std::io::Error::other("MCP response deadline"))?
            };
            let Some(chunk) = item else { break; };
            let chunk = chunk.map_err(|_| std::io::Error::other("MCP response interrupted"))?;
            budget.consume(&chunk)?;
            if !chunk.is_empty() { yield chunk; }
        }
    };
    let events = SseStream::from_bytes_stream(
        stream.map(|item: Result<axum::body::Bytes, std::io::Error>| item),
    );
    async_stream::try_stream! {
        futures::pin_mut!(events);
        let mut count = 0usize;
        let mut window = tokio::time::Instant::now();
        while let Some(event) = events.next().await {
            if mode.subscription() && window.elapsed() >= Duration::from_secs(60) {
                window = tokio::time::Instant::now();
                count = 0;
            }
            count += 1;
            if count > 256 {
                Err(SseError::Body(Box::new(std::io::Error::other("MCP SSE event count budget"))))?;
            }
            yield event?;
        }
    }
    .boxed()
}
async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, HttpError> {
    tokio::time::timeout(Duration::from_secs(30), request.send())
        .await
        .map_err(|_| failure("MCP HTTP response headers deadline"))?
        .map_err(|_| failure("MCP HTTP request failed"))
}
async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, HttpError> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_MESSAGE_BYTES as u64)
    {
        return Err(failure("MCP body byte budget"));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let next = tokio::time::timeout_at(deadline, stream.next())
            .await
            .map_err(|_| failure("MCP body deadline"))?;
        let Some(chunk) = next else {
            return Ok(body);
        };
        let chunk = chunk.map_err(|_| failure("MCP body interrupted"))?;
        if chunk.len() > MAX_MESSAGE_BYTES.saturating_sub(body.len()) {
            return Err(failure("MCP body byte budget"));
        }
        body.extend_from_slice(&chunk);
    }
}
impl StreamableHttpClient for BoundedHttp {
    type Error = std::io::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let mode = if matches!(&message, ClientJsonRpcMessage::Request(r) if matches!(r.request, ClientRequest::SubscriptionsListenRequest(_)))
        {
            EventMode::Subscription
        } else {
            EventMode::Response
        };
        let body = serde_json::to_vec(&message)?;
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(failure("MCP request byte budget"));
        }
        let response = send(
            self.request(
                reqwest::Method::POST,
                &uri,
                session.as_deref(),
                auth,
                headers,
            )?
            .header("content-type", "application/json")
            .body(body),
        )
        .await?;
        let status = response.status();
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        if status == reqwest::StatusCode::ACCEPTED {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // Do not return an auth challenge to an automatic OAuth retry wrapper.
        if !status.is_success() {
            let _ = bounded_body(response).await?;
            return Err(failure(&format!(
                "MCP HTTP status {status}; request was not retried"
            )));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if content_type.starts_with("text/event-stream") {
            return Ok(StreamableHttpPostResponse::Sse(
                bounded_events(response, mode),
                session,
            ));
        }
        if !content_type.starts_with("application/json") {
            return Err(failure("MCP response content type"));
        }
        let body = bounded_body(response).await?;
        let message: ServerJsonRpcMessage = serde_json::from_slice(&body)?;
        Ok(StreamableHttpPostResponse::Json(message, session))
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last: Option<String>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, HttpError> {
        let mut request = self.request(
            reqwest::Method::GET,
            &uri,
            session.as_deref(),
            auth,
            headers,
        )?;
        if let Some(last) = last {
            request = request.header("last-event-id", last);
        }
        let response = send(request).await?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(HttpError::ServerDoesNotSupportSse);
        }
        if !response.status().is_success() {
            let _ = bounded_body(response).await?;
            return Err(failure("MCP event stream rejected"));
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
        {
            return Err(failure("MCP event content type"));
        }
        Ok(bounded_events(response, EventMode::Subscription))
    }
    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), HttpError> {
        let response =
            send(self.request(reqwest::Method::DELETE, &uri, Some(&session), auth, headers)?)
                .await?;
        let status = response.status();
        let _ = bounded_body(response).await?;
        if status.is_success() || status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            Ok(())
        } else {
            Err(failure("MCP session close rejected"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        response::{IntoResponse, Response},
        routing::get,
    };
    async fn fixture(
        path: &'static str,
        response: impl Fn() -> Response + Clone + Send + Sync + 'static,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{}", listener.local_addr().unwrap(), path);
        let app = Router::new().route(
            path,
            get(move || {
                let response = response.clone();
                async move { response() }
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, task)
    }
    #[tokio::test(start_paused = true)]
    async fn subscription_windows_never_reset_unfinished_frame_budget() {
        let mut partial = EventBudget::new(EventMode::Subscription);
        partial.consume(&vec![b'x'; MAX_MESSAGE_BYTES]).unwrap();
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(partial.consume(b"x").is_err());

        let mut complete = EventBudget::new(EventMode::Subscription);
        complete.consume(b"data: ok\r").unwrap();
        complete.consume(b"\n\r").unwrap();
        complete.consume(b"\ndata: again\n\n").unwrap();
        assert_eq!(complete.frame, 0);
        complete
            .consume(&vec![b'x'; MAX_MESSAGE_BYTES - 32])
            .unwrap();
        complete.consume(b"\n\n").unwrap();
        tokio::time::advance(Duration::from_secs(61)).await;
        complete.consume(&vec![b'x'; MAX_MESSAGE_BYTES]).unwrap();

        let mut response = EventBudget::new(EventMode::Response);
        response.consume(&vec![b'\n'; MAX_MESSAGE_BYTES]).unwrap();
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(response.consume(b"\n").is_err());
    }
    #[tokio::test]
    async fn bounded_http_rejects_oversized_json_comments_and_event_floods() {
        let (url, task) = fixture("/large", || {
            "x".repeat(MAX_MESSAGE_BYTES + 1).into_response()
        })
        .await;
        let response = reqwest::get(&url).await.unwrap();
        assert!(bounded_body(response).await.is_err());
        task.abort();
        let _ = task.await;
        let (url, task) = fixture("/comments", || {
            (
                [("content-type", "text/event-stream")],
                format!(":{}\n\n", "x".repeat(MAX_MESSAGE_BYTES)),
            )
                .into_response()
        })
        .await;
        let mut stream = bounded_events(reqwest::get(&url).await.unwrap(), EventMode::Response);
        assert!(stream.next().await.unwrap().is_err());
        task.abort();
        let _ = task.await;
        let (url, task) = fixture("/events", || {
            (
                [("content-type", "text/event-stream")],
                "data: {}\n\n".repeat(257),
            )
                .into_response()
        })
        .await;
        let mut stream = bounded_events(reqwest::get(&url).await.unwrap(), EventMode::Response);
        for _ in 0..256 {
            assert!(stream.next().await.unwrap().is_ok());
        }
        assert!(stream.next().await.unwrap().is_err());
        task.abort();
        let _ = task.await;
    }
    #[tokio::test]
    async fn bounded_http_never_follows_redirect_or_sends_credentials_to_other_endpoint() {
        let (url, task) = fixture("/redirect", || {
            (
                axum::http::StatusCode::TEMPORARY_REDIRECT,
                [("location", "http://127.0.0.1:1/secret")],
            )
                .into_response()
        })
        .await;
        let client = BoundedHttp::new(&url).unwrap();
        let response = client
            .request(
                reqwest::Method::GET,
                &url,
                None,
                Some("test-token".into()),
                HashMap::new(),
            )
            .unwrap()
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert!(
            client
                .request(
                    reqwest::Method::GET,
                    "http://127.0.0.1:1/secret",
                    None,
                    Some("test-token".into()),
                    HashMap::new()
                )
                .is_err()
        );
        task.abort();
        let _ = task.await;
    }
}
