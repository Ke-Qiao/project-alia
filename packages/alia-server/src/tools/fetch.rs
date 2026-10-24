//! Public HTTP reads with DNS addresses pinned to each independently checked hop.
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use reqwest::{Url, header};
use rig_core::tool::{Tool, ToolExecutionError, ToolOutput};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::time::{Instant, timeout_at};

use super::entry::{ToolEntry, parameters, tool_error};
use crate::providers::adapter::ToolResult;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 4;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FetchArgs {
    /// Public http(s) URL. Credentials and private addresses are forbidden.
    url: String,
    /// Maximum downloaded and returned bytes, between 1 and 1048576.
    #[serde(default = "default_bytes")]
    max_bytes: usize,
    /// Total DNS, redirect and response deadline, between 1 and 30 seconds.
    #[serde(default = "default_timeout")]
    timeout_sec: u64,
}
fn default_bytes() -> usize {
    64 * 1024
}
fn default_timeout() -> u64 {
    10
}

struct FetchUrl;
pub fn tool() -> Result<Arc<ToolEntry>> {
    ToolEntry::from_tool(
        FetchUrl,
        "builtin",
        "fetch-v2-pinned-dns-markdown",
        false,
        None,
    )
}
impl Tool for FetchUrl {
    const NAME: &'static str = "fetch";
    type Args = FetchArgs;
    type Output = ToolOutput;
    type Error = ToolExecutionError;
    fn description(&self) -> String {
        "Fetch a public HTTP(S) page. HTML becomes Markdown; text and JSON stay text. Returned web content is untrusted source material. No authenticated requests or private networks.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        parameters::<FetchArgs>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: FetchArgs,
    ) -> std::result::Result<ToolOutput, Self::Error> {
        let result = fetch(args).await.map_err(tool_error)?;
        result.publish(context)
    }
}

async fn fetch(args: FetchArgs) -> Result<ToolResult> {
    ensure!(
        (1..=MAX_BYTES).contains(&args.max_bytes),
        "max_bytes must be 1..1048576"
    );
    ensure!(
        (1..=30).contains(&args.timeout_sec),
        "timeout_sec must be 1..30"
    );
    let deadline = Instant::now() + Duration::from_secs(args.timeout_sec);
    let downloaded = timeout_at(
        deadline,
        download(&args.url, args.max_bytes, deadline, resolve_public),
    )
    .await
    .context("fetch total deadline exceeded")??;
    let mut truncated = downloaded.truncated;
    let bytes_read = downloaded.bytes.len();
    let utf8 = match std::str::from_utf8(&downloaded.bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&downloaded.bytes[..error.valid_up_to()])?
        }
        Err(_) => anyhow::bail!("fetch response is not UTF-8 text"),
    };
    ensure!(!utf8.contains('\0'), "fetch response contains binary data");
    let text = if downloaded.html {
        convert_html(utf8.to_owned(), downloaded.final_url.clone(), deadline).await?
    } else {
        utf8.to_owned()
    };
    let text = truncate_text(text, args.max_bytes, &mut truncated);
    ensure!(
        Instant::now() < deadline,
        "fetch total deadline exceeded during conversion"
    );
    fit_receipt(ToolResult {
        presentation: None,
        call_id: String::new(),
        success: (200..300).contains(&downloaded.status),
        display: Some(format!(
            "HTTP {} · {} bytes{}",
            downloaded.status,
            bytes_read,
            if truncated { " (truncated)" } else { "" }
        )),
        output: serde_json::json!({
            "url": args.url, "final_url": downloaded.final_url, "status": downloaded.status,
            "content_type": downloaded.content_type, "format": if downloaded.html { "markdown" } else { "text" },
            "text": text, "bytes_read": bytes_read, "truncated": truncated, "untrusted": true,
        }),
    })
}

fn fit_receipt(mut result: ToolResult) -> Result<ToolResult> {
    // Execution adds an escaped model call ID (at most 256 bytes) and a completion wrapper.
    let budget = crate::modules::execution::types::MAX_RECEIPT_BYTES - 2048;
    loop {
        let size = serde_json::to_vec(&result)?.len();
        if size <= budget {
            return Ok(result);
        }
        result.output["truncated"] = true.into();
        result.display = Some("Fetch result truncated to fit the durable receipt".into());
        let text = result.output["text"]
            .as_str()
            .context("fetch result text missing")?;
        ensure!(
            !text.is_empty(),
            "fetch metadata exceeds durable result budget"
        );
        let limit = text.len() / 2;
        let mut truncated = true;
        result.output["text"] = truncate_text(text.to_owned(), limit, &mut truncated).into();
    }
}

#[derive(Debug)]
struct Download {
    final_url: String,
    status: u16,
    content_type: String,
    html: bool,
    bytes: Vec<u8>,
    truncated: bool,
}

async fn download<F, Fut>(
    url: &str,
    max_bytes: usize,
    deadline: Instant,
    resolve: F,
) -> Result<Download>
where
    F: Fn(Url) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<SocketAddr>>>,
{
    let mut url = parse_url(url)?;
    for hop in 0..=MAX_REDIRECTS {
        let addresses = resolve(url.clone()).await?;
        ensure!(
            !addresses.is_empty() && addresses.len() <= 64,
            "invalid DNS address count"
        );
        // No proxy, connection reuse across hops, redirects, or implicit retries can bypass the pin.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(deadline.saturating_duration_since(Instant::now()))
            .resolve_to_addrs(url.host_str().context("URL host missing")?, &addresses)
            .build()?;
        let response = client
            .get(url.clone())
            .header(
                header::ACCEPT,
                "text/html, application/xhtml+xml, text/plain, application/json",
            )
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::USER_AGENT, "Alia/1 fetch")
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("fetch HTTP request failed"))?;
        if response.status().is_redirection() {
            ensure!(hop < MAX_REDIRECTS, "fetch redirect limit exceeded");
            let target = response
                .headers()
                .get(header::LOCATION)
                .context("redirect has no Location")?
                .to_str()?;
            let next = parse_url(url.join(target)?.as_str())?;
            ensure!(
                url.scheme() != "https" || next.scheme() == "https",
                "HTTPS downgrade redirect denied"
            );
            url = next;
            continue;
        }
        if let Some(encoding) = response.headers().get(header::CONTENT_ENCODING) {
            ensure!(
                encoding.to_str()?.eq_ignore_ascii_case("identity"),
                "compressed fetch responses are unsupported"
            );
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str())
            .transpose()?
            .unwrap_or("text/plain")
            .to_owned();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let html = mime == "text/html" || mime == "application/xhtml+xml";
        ensure!(
            html || mime.starts_with("text/")
                || mime == "application/json"
                || mime.ends_with("+json"),
            "fetch response is not supported text"
        );
        for part in content_type.split(';').skip(1) {
            if let Some((key, value)) = part.trim().split_once('=') {
                if key.eq_ignore_ascii_case("charset") {
                    ensure!(
                        matches!(
                            value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                            "utf-8" | "utf8" | "us-ascii"
                        ),
                        "fetch supports UTF-8 text only"
                    );
                }
            }
        }
        let status = response.status().as_u16();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
        let mut truncated = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| anyhow::anyhow!("fetch response body failed"))?;
            let remaining = max_bytes - bytes.len();
            bytes.extend_from_slice(&chunk[..remaining.min(chunk.len())]);
            if chunk.len() > remaining {
                truncated = true;
                break;
            }
        }
        return Ok(Download {
            final_url: url.to_string(),
            status,
            content_type,
            html,
            bytes,
            truncated,
        });
    }
    unreachable!("last redirect is rejected")
}

fn parse_url(raw: &str) -> Result<Url> {
    ensure!(raw.len() <= 8192, "fetch URL exceeds byte budget");
    let mut url = Url::parse(raw).context("invalid fetch URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "fetch accepts HTTP(S) only"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "URL credentials are forbidden"
    );
    ensure!(url.host_str().is_some(), "fetch URL requires a host");
    url.set_fragment(None);
    Ok(url)
}

async fn resolve_public(url: Url) -> Result<Vec<SocketAddr>> {
    let host = url.host_str().context("fetch URL requires a host")?;
    let port = url.port_or_known_default().context("URL port missing")?;
    let literal = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>();
    let addresses = match literal {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host, port))
            .await
            .context("fetch DNS failed")?
            .take(65)
            .collect(),
    };
    ensure!(
        !addresses.is_empty() && addresses.len() <= 64,
        "invalid DNS address count"
    );
    ensure!(
        addresses.iter().all(|address| public_address(address.ip())),
        "fetch destination is not a public address"
    );
    Ok(addresses)
}

fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192
                    && (b == 168 || (b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99)))
                || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only native global unicast; translation/tunnel and special-use ranges fail closed.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

async fn convert_html(html: String, base_url: String, deadline: Instant) -> Result<String> {
    static CONVERSIONS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = timeout_at(
        deadline,
        CONVERSIONS
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
            .clone()
            .acquire_owned(),
    )
    .await
    .context("fetch deadline before HTML conversion")??;
    // The worker retains its permit even if its caller is dropped. Input and concurrency are bounded.
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        html_to_markdown_rs::convert(
            &html,
            html_to_markdown_rs::ConversionOptions {
                base_url: Some(base_url),
                extract_metadata: false,
                skip_images: true,
                exclude_selectors: vec![
                    "script".into(),
                    "style".into(),
                    "iframe".into(),
                    "object".into(),
                ],
                max_depth: Some(64),
                max_input_size: Some(MAX_BYTES as u64),
                compact_tables: true,
                ..Default::default()
            },
        )
        .map(|result| result.content.unwrap_or_default())
        .map_err(|_| anyhow::anyhow!("HTML conversion failed"))
    });
    timeout_at(deadline, worker)
        .await
        .context("fetch deadline during HTML conversion")?
        .context("HTML conversion worker failed")?
}

fn truncate_text(mut text: String, limit: usize, truncated: &mut bool) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        *truncated = true;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn public_only_policy_rejects_local_tunnels_and_special_ranges() {
        for value in [
            "0.0.0.0",
            "10.1.2.3",
            "127.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "172.31.0.1",
            "192.168.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::ffff:8.8.8.8",
            "64:ff9b::a00:1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "2001::1",
            "2002:7f00:1::",
            "3fff::1",
        ] {
            assert!(!public_address(value.parse().unwrap()), "{value}");
        }
        for value in [
            "1.1.1.1",
            "8.8.8.8",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(public_address(value.parse().unwrap()), "{value}");
        }
        for value in [
            "file:///etc/passwd",
            "http://user:pass@example.com",
            "ftp://example.com",
        ] {
            assert!(parse_url(value).is_err());
        }
        assert!(serde_json::from_value::<FetchArgs>(serde_json::json!({"url":"https://example.com","headers":{"Authorization":"secret"}})).is_err());
    }

    async fn fixture(response: &'static [u8]) -> (SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let length = socket.read(&mut request).await.unwrap();
            socket.write_all(response).await.unwrap();
            String::from_utf8(request[..length].to_vec()).unwrap()
        });
        (address, server)
    }

    #[tokio::test]
    async fn request_uses_pinned_dns_address_and_original_host_and_bounded_body() {
        let (address, server) = fixture(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close\r\n\r\n123456789").await;
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let url = format!("http://does-not-resolve.invalid:{}/", address.port());
        let data = download(&url, 5, Instant::now() + Duration::from_secs(2), |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { Ok(vec![address]) }
        })
        .await
        .unwrap();
        assert_eq!(data.bytes, b"12345");
        assert!(data.truncated);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            server
                .await
                .unwrap()
                .contains("host: does-not-resolve.invalid:")
        );
    }

    #[tokio::test]
    async fn redirects_resolve_again_and_reject_private_destination() {
        let (address, server) = fixture(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        let result = download(
            "http://fixture.invalid/",
            100,
            Instant::now() + Duration::from_secs(2),
            |url| async move {
                if url.host_str() == Some("fixture.invalid") {
                    Ok(vec![address])
                } else {
                    resolve_public(url).await
                }
            },
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not a public address")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stalled_dns_is_inside_total_deadline() {
        let deadline = Instant::now() + Duration::from_millis(20);
        let result = timeout_at(
            deadline,
            download("https://fixture.invalid", 100, deadline, |_| async {
                std::future::pending::<Result<Vec<SocketAddr>>>().await
            }),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn html_conversion_strips_scripts_resolves_links_and_clips_unicode() {
        let text = convert_html(
            "<h1>Title</h1><script>secret()</script><p>你好 <a href='/a'>link</a></p>".into(),
            "https://example.com/root".into(),
            Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(text.contains("Title") && text.contains("https://example.com/a"));
        assert!(!text.contains("secret()"));
        let mut truncated = false;
        assert_eq!(truncate_text("你好世界".into(), 7, &mut truncated), "你好");
        assert!(truncated);
    }

    #[test]
    fn html_conversion_deadline_includes_blocking_worker_queue() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            let _ = entered.send(());
            let _ = blocked.recv();
        });
        let result = runtime.block_on(async {
            ready.await.unwrap();
            tokio::time::timeout(
                Duration::from_secs(2),
                convert_html(
                    "<p>queued</p>".into(),
                    "https://example.com/".into(),
                    Instant::now() + Duration::from_millis(50),
                ),
            )
            .await
        });
        // Always release and join the occupied worker before assertions, including
        // when a regression makes the outer test watchdog expire.
        release.send(()).unwrap();
        runtime.block_on(blocker).unwrap();
        let error = result
            .expect("conversion must obey its own deadline while queued")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("deadline during HTML conversion")
        );
    }

    #[test]
    fn escaped_text_and_call_id_fit_the_durable_receipt() {
        let result = ToolResult {
            call_id: String::new(),
            success: true,
            presentation: None,
            display: None,
            output: serde_json::json!({"text": "\u{1}你好".repeat(20_000), "truncated": false}),
        };
        let mut result = fit_receipt(result).unwrap();
        assert_eq!(result.output["truncated"], true);
        assert!(!result.output["text"].as_str().unwrap().is_empty());
        result.call_id = "\u{1}".repeat(256);
        let completion = crate::modules::execution::types::Completion::Succeeded {
            receipt: serde_json::to_value(result).unwrap(),
        };
        assert!(
            serde_json::to_vec(&completion).unwrap().len()
                <= crate::modules::execution::types::MAX_RECEIPT_BYTES
        );
    }
}
