use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

/// Egress policy defining permitted outbound network destinations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// Permitted destinations in `host:port` or `host` format (e.g. `["api.openai.com:443", "*.github.com:443"]`).
    pub allow_hosts: Vec<String>,
}

impl EgressPolicy {
    pub fn new(allow_hosts: Vec<String>) -> Self {
        Self { allow_hosts }
    }

    /// Determines whether an outbound request to `(target_host, target_port)` is permitted.
    ///
    /// Rules:
    /// 1. Cloud metadata endpoints (`169.254.169.254`, `169.254.*`) are strictly denied unless explicitly permitted.
    /// 2. If `allow_hosts` is empty, egress is unconstrained (standard container networking).
    /// 3. If `allow_hosts` is non-empty, all unlisted destinations are denied (default-deny egress).
    pub fn is_allowed(&self, target_host: &str, target_port: u16) -> bool {
        // Strict guard against cloud instance metadata exfiltration
        if target_host == "169.254.169.254" || target_host.starts_with("169.254.") {
            return self.matches_any(target_host, target_port);
        }

        if self.allow_hosts.is_empty() {
            return true;
        }

        self.matches_any(target_host, target_port)
    }

    fn matches_any(&self, target_host: &str, target_port: u16) -> bool {
        for rule in &self.allow_hosts {
            if let Some((rule_host, rule_port_str)) = rule.split_once(':') {
                if let Ok(rule_port) = rule_port_str.parse::<u16>() {
                    if rule_port == target_port && Self::host_matches(rule_host, target_host) {
                        return true;
                    }
                }
            } else if Self::host_matches(rule, target_host) {
                return true;
            }
        }
        false
    }

    fn host_matches(pattern: &str, host: &str) -> bool {
        let pattern = pattern.to_lowercase();
        let host = host.to_lowercase();
        if pattern == host {
            return true;
        }
        if let Some(suffix) = pattern.strip_prefix("*.") {
            if host.ends_with(suffix)
                && host.len() > suffix.len()
                && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            {
                return true;
            }
        }
        false
    }
}

/// Zero-trust secret substitution engine.
///
/// Replaces placeholder tokens (e.g. `krun-secret:OPENAI_API_KEY`) with real credentials
/// in-flight during outbound proxy requests.
#[derive(Debug, Clone, Default)]
pub struct SecretSubstitution {
    mapping: Vec<(String, String)>,
}

impl SecretSubstitution {
    pub fn new(secrets: &[(String, String)]) -> Self {
        let mut mapping = Vec::new();
        for (key, val) in secrets {
            let placeholder = format!("krun-secret:{}", key);
            mapping.push((placeholder, val.clone()));
        }
        Self { mapping }
    }

    /// Replaces placeholder occurrences inside an HTTP request buffer.
    pub fn substitute(&self, buffer: &[u8]) -> Vec<u8> {
        if self.mapping.is_empty() {
            return buffer.to_vec();
        }

        if let Ok(mut text) = std::str::from_utf8(buffer).map(|s| s.to_string()) {
            for (placeholder, real_val) in &self.mapping {
                if text.contains(placeholder) {
                    text = text.replace(placeholder, real_val);
                }
            }
            text.into_bytes()
        } else {
            buffer.to_vec()
        }
    }
}

/// LLM Token Metering and Hard Budget Enforcer.
#[derive(Debug, Default)]
pub struct LlmTokenBudget {
    max_tokens: Option<u64>,
    consumed_tokens: AtomicU64,
}

impl LlmTokenBudget {
    pub fn new(max_tokens: Option<u64>) -> Self {
        Self {
            max_tokens,
            consumed_tokens: AtomicU64::new(0),
        }
    }

    pub fn is_exceeded(&self) -> bool {
        if let Some(max) = self.max_tokens {
            self.consumed_tokens.load(Ordering::Relaxed) >= max
        } else {
            false
        }
    }

    pub fn record_tokens(&self, count: u64) {
        self.consumed_tokens.fetch_add(count, Ordering::Relaxed);
    }

    pub fn total_consumed(&self) -> u64 {
        self.consumed_tokens.load(Ordering::Relaxed)
    }

    /// Inspects HTTP response payloads (both standard JSON bodies and SSE stream lines)
    /// to extract and count token usage.
    pub fn inspect_chunk(&self, chunk: &[u8]) {
        if let Ok(text) = std::str::from_utf8(chunk) {
            for line in text.lines() {
                let trimmed = line.trim();
                let json_candidate = if let Some(rest) = trimmed.strip_prefix("data: ") {
                    rest.trim()
                } else if trimmed.starts_with('{') {
                    trimmed
                } else {
                    continue;
                };

                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_candidate) {
                    if let Some(usage) = val.get("usage") {
                        if let Some(total) = usage.get("total_tokens").and_then(|t| t.as_u64()) {
                            self.record_tokens(total);
                        }
                    }
                }
            }
        }
    }
}

/// Embedded host-side egress HTTP/CONNECT proxy server.
pub struct EgressProxyServer {
    local_addr: SocketAddr,
    policy: Arc<EgressPolicy>,
    secrets: Arc<SecretSubstitution>,
    budget: Arc<LlmTokenBudget>,
    blocked_requests: Arc<AtomicU64>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl EgressProxyServer {
    /// Binds to an ephemeral localhost address (`127.0.0.1:0`) and starts the proxy loop.
    pub async fn start(
        policy: EgressPolicy,
        secrets: SecretSubstitution,
        budget: LlmTokenBudget,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Failed to bind egress proxy server")?;
        let local_addr = listener.local_addr()?;

        let policy = Arc::new(policy);
        let secrets = Arc::new(secrets);
        let budget = Arc::new(budget);
        let blocked_requests = Arc::new(AtomicU64::new(0));

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let p_clone = policy.clone();
        let s_clone = secrets.clone();
        let b_clone = budget.clone();
        let bl_clone = blocked_requests.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((client, _peer)) => {
                                let pol = p_clone.clone();
                                let sec = s_clone.clone();
                                let bud = b_clone.clone();
                                let blk = bl_clone.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_client(client, pol, sec, bud, blk).await {
                                        tracing::debug!("Egress proxy connection closed: {e}");
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::warn!("Egress proxy accept error: {e}");
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("Egress proxy server shutting down");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            policy,
            secrets,
            budget,
            blocked_requests,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn proxy_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    pub fn policy(&self) -> &EgressPolicy {
        &self.policy
    }

    pub fn secrets(&self) -> &SecretSubstitution {
        &self.secrets
    }

    pub fn blocked_count(&self) -> u64 {
        self.blocked_requests.load(Ordering::Relaxed)
    }

    pub fn tokens_consumed(&self) -> u64 {
        self.budget.total_consumed()
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for EgressProxyServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn find_header_end(buf: &[u8]) -> Option<(usize, usize)> {
    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some((pos, pos + 4))
    } else {
        buf.windows(2)
            .position(|w| w == b"\n\n")
            .map(|pos| (pos, pos + 2))
    }
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        if line.to_ascii_lowercase().starts_with("content-length:") {
            if let Some((_, val)) = line.split_once(':') {
                return val.trim().parse::<usize>().ok();
            }
        }
    }
    None
}

fn adjust_content_length(headers: &str, new_body_len: usize) -> String {
    let mut updated_lines = Vec::new();
    let mut found_cl = false;
    for line in headers.lines() {
        if line.to_ascii_lowercase().starts_with("content-length:") {
            updated_lines.push(format!("Content-Length: {new_body_len}"));
            found_cl = true;
        } else {
            updated_lines.push(line.to_string());
        }
    }
    if !found_cl && new_body_len > 0 {
        updated_lines.push(format!("Content-Length: {new_body_len}"));
    }
    updated_lines.join("\r\n") + "\r\n\r\n"
}

async fn handle_client(
    mut client: TcpStream,
    policy: Arc<EgressPolicy>,
    secrets: Arc<SecretSubstitution>,
    budget: Arc<LlmTokenBudget>,
    blocked_requests: Arc<AtomicU64>,
) -> Result<()> {
    let mut buf = Vec::with_capacity(8192);
    let mut temp = [0u8; 8192];

    // 1. Dynamically read headers until \r\n\r\n or \n\n
    let (header_end, body_start) = loop {
        let n = client.read(&mut temp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&temp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            bail!("HTTP request headers exceed maximum size (64 KiB)");
        }
    };

    let request_head = String::from_utf8_lossy(&buf[..header_end]);
    let first_line = request_head.lines().next().unwrap_or_default();
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("Malformed HTTP request from client");
    }

    let method = parts[0];
    let target = parts[1];

    if method.eq_ignore_ascii_case("CONNECT") {
        // HTTPS tunneling: CONNECT target:port HTTP/1.1
        let (host, port) = parse_host_port(target, 443)?;

        if budget.is_exceeded() {
            blocked_requests.fetch_add(1, Ordering::Relaxed);
            let resp = b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 32\r\n\r\nLLM token budget limit exceeded\r\n";
            client.write_all(resp).await?;
            return Ok(());
        }

        if !policy.is_allowed(&host, port) {
            blocked_requests.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("Egress DENIED by policy: {}:{}", host, port);
            let resp = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 46\r\n\r\nBlocked by krun-microvm default-deny policy\r\n";
            client.write_all(resp).await?;
            return Ok(());
        }

        // Establish connection to upstream
        let mut upstream = TcpStream::connect((host.as_str(), port))
            .await
            .with_context(|| format!("Failed to connect to target {}:{}", host, port))?;

        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;

        // Forward any extra bytes that were already read past CONNECT headers
        if buf.len() > body_start {
            upstream.write_all(&buf[body_start..]).await?;
        }

        tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    } else {
        // Plain HTTP forward proxying
        let (host, port, _path) = parse_http_target(target)?;

        if budget.is_exceeded() {
            blocked_requests.fetch_add(1, Ordering::Relaxed);
            let resp = b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 32\r\n\r\nLLM token budget limit exceeded\r\n";
            client.write_all(resp).await?;
            return Ok(());
        }

        if !policy.is_allowed(&host, port) {
            blocked_requests.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("Egress DENIED by policy: {}:{}", host, port);
            let resp = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 46\r\n\r\nBlocked by krun-microvm default-deny policy\r\n";
            client.write_all(resp).await?;
            return Ok(());
        }

        // Read complete body if Content-Length is present
        let content_len = parse_content_length(&request_head);
        if let Some(expected_len) = content_len {
            if expected_len > 32 * 1024 * 1024 {
                bail!("Request body exceeds maximum allowed size (32 MiB)");
            }
            while buf.len() < body_start + expected_len {
                let n = client.read(&mut temp).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&temp[..n]);
            }
        }

        // Apply secret substitution on outbound headers and body
        let headers_raw = &buf[..header_end];
        let body_raw = &buf[body_start..];

        let substituted_headers = secrets.substitute(headers_raw);
        let substituted_body = secrets.substitute(body_raw);

        let final_req = if content_len.is_some() {
            let headers_str = String::from_utf8_lossy(&substituted_headers);
            let updated_headers = adjust_content_length(&headers_str, substituted_body.len());
            let mut out = updated_headers.into_bytes();
            out.extend_from_slice(&substituted_body);
            out
        } else {
            let mut out = substituted_headers;
            out.extend_from_slice(b"\r\n\r\n");
            out.extend_from_slice(&substituted_body);
            out
        };

        let mut upstream = TcpStream::connect((host.as_str(), port))
            .await
            .with_context(|| format!("Failed to connect to HTTP target {}:{}", host, port))?;

        upstream.write_all(&final_req).await?;

        // Forward response back and monitor token usage
        let mut resp_buf = [0u8; 8192];
        loop {
            let r = upstream.read(&mut resp_buf).await?;
            if r == 0 {
                break;
            }
            budget.inspect_chunk(&resp_buf[..r]);
            client.write_all(&resp_buf[..r]).await?;
        }
    }

    Ok(())
}

fn parse_host_port(target: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some((h, p)) = target.split_once(':') {
        let port = p.parse::<u16>().unwrap_or(default_port);
        Ok((h.to_string(), port))
    } else {
        Ok((target.to_string(), default_port))
    }
}

fn parse_http_target(target: &str) -> Result<(String, u16, String)> {
    if let Some(rest) = target.strip_prefix("http://") {
        let (host_part, path) = if let Some(slash_pos) = rest.find('/') {
            (&rest[..slash_pos], rest[slash_pos..].to_string())
        } else {
            (rest, "/".to_string())
        };
        let (host, port) = parse_host_port(host_part, 80)?;
        Ok((host, port, path))
    } else {
        let (host, port) = parse_host_port(target, 80)?;
        Ok((host, port, "/".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_egress_policy_allow_and_deny() {
        let policy = EgressPolicy::new(vec![
            "api.openai.com:443".to_string(),
            "pypi.org:443".to_string(),
            "files.pythonhosted.org:443".to_string(),
        ]);

        assert!(policy.is_allowed("api.openai.com", 443));
        assert!(policy.is_allowed("pypi.org", 443));
        assert!(policy.is_allowed("files.pythonhosted.org", 443));

        // Wrong port
        assert!(!policy.is_allowed("api.openai.com", 80));

        // Unlisted host
        assert!(!policy.is_allowed("malicious.site", 443));
        assert!(!policy.is_allowed("evil.com", 80));
    }

    #[test]
    fn test_egress_policy_wildcard_matching() {
        let policy = EgressPolicy::new(vec!["*.github.com:443".to_string()]);

        assert!(policy.is_allowed("api.github.com", 443));
        assert!(policy.is_allowed("raw.github.com", 443));
        assert!(policy.is_allowed("codeload.github.com", 443));

        // Wrong domain or base without dot
        assert!(!policy.is_allowed("notgithub.com", 443));
        assert!(!policy.is_allowed("github.com", 443));
    }

    #[test]
    fn test_egress_policy_blocks_metadata() {
        // Even when policy allows all (empty allow_hosts), metadata is blocked!
        let open_policy = EgressPolicy::default();
        assert!(!open_policy.is_allowed("169.254.169.254", 80));
        assert!(!open_policy.is_allowed("169.254.1.1", 80));

        // Only allowed if explicitly in rules
        let explicit_policy = EgressPolicy::new(vec!["169.254.169.254:80".to_string()]);
        assert!(explicit_policy.is_allowed("169.254.169.254", 80));
    }

    #[test]
    fn test_secret_substitution_header_and_body() {
        let secrets = vec![
            (
                "OPENAI_API_KEY".to_string(),
                "sk-proj-supersecret123".to_string(),
            ),
            ("GH_TOKEN".to_string(), "ghp_realtoken456".to_string()),
        ];
        let engine = SecretSubstitution::new(&secrets);

        let input = b"GET /v1/chat HTTP/1.1\r\nAuthorization: Bearer krun-secret:OPENAI_API_KEY\r\nX-Hub: krun-secret:GH_TOKEN\r\n\r\n";
        let substituted = engine.substitute(input);
        let out = String::from_utf8(substituted).unwrap();

        assert!(!out.contains("krun-secret:OPENAI_API_KEY"));
        assert!(!out.contains("krun-secret:GH_TOKEN"));
        assert!(out.contains("Authorization: Bearer sk-proj-supersecret123"));
        assert!(out.contains("X-Hub: ghp_realtoken456"));
    }

    #[test]
    fn test_llm_token_budget_enforcement() {
        let budget = LlmTokenBudget::new(Some(100));
        assert!(!budget.is_exceeded());

        // Parse standard json usage
        let chunk1 = br#"{"id":"chat1","usage":{"prompt_tokens":30,"completion_tokens":20,"total_tokens":50}}"#;
        budget.inspect_chunk(chunk1);
        assert_eq!(budget.total_consumed(), 50);
        assert!(!budget.is_exceeded());

        // Parse SSE streaming chunk
        let chunk2 = b"data: {\"usage\":{\"total_tokens\":60}}\n\n";
        budget.inspect_chunk(chunk2);
        assert_eq!(budget.total_consumed(), 110);
        assert!(budget.is_exceeded());
    }

    #[tokio::test]
    async fn test_egress_proxy_connect_denied() {
        let policy = EgressPolicy::new(vec!["api.openai.com:443".to_string()]);
        let secrets = SecretSubstitution::default();
        let budget = LlmTokenBudget::default();

        let proxy = EgressProxyServer::start(policy, secrets, budget)
            .await
            .unwrap();

        let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy.port()))
            .await
            .unwrap();
        client
            .write_all(b"CONNECT forbidden.com:443 HTTP/1.1\r\nHost: forbidden.com:443\r\n\r\n")
            .await
            .unwrap();

        let mut resp = [0u8; 128];
        let n = client.read(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp[..n]);

        assert!(resp_str.contains("403 Forbidden"));
        assert_eq!(proxy.blocked_count(), 1);
    }

    #[test]
    fn test_adjust_content_length() {
        let headers = "POST /v1/chat HTTP/1.1\r\nHost: api.openai.com\r\nContent-Length: 10";
        let adjusted = adjust_content_length(headers, 42);
        assert!(adjusted.contains("Content-Length: 42"));
        assert!(!adjusted.contains("Content-Length: 10"));
        assert!(adjusted.ends_with("\r\n\r\n"));
    }

    #[tokio::test]
    async fn test_dynamic_buffering_and_secret_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut temp = [0u8; 1024];
            let (h_end, b_start) = loop {
                let n = stream.read(&mut temp).await.unwrap();
                buf.extend_from_slice(&temp[..n]);
                if let Some(pos) = find_header_end(&buf) {
                    break pos;
                }
            };
            let headers = String::from_utf8_lossy(&buf[..h_end]);
            let cl = parse_content_length(&headers).unwrap();
            while buf.len() < b_start + cl {
                let n = stream.read(&mut temp).await.unwrap();
                buf.extend_from_slice(&temp[..n]);
            }
            let body = &buf[b_start..];
            assert_eq!(body.len(), cl);
            assert!(String::from_utf8_lossy(body).contains("sk-proj-actual-long-key-999"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                .await
                .unwrap();
        });

        let policy = EgressPolicy::new(vec![format!("127.0.0.1:{}", upstream_addr.port())]);
        let secrets = SecretSubstitution::new(&[(
            "API_KEY".to_string(),
            "sk-proj-actual-long-key-999".to_string(),
        )]);
        let budget = LlmTokenBudget::default();

        let proxy = EgressProxyServer::start(policy, secrets, budget)
            .await
            .unwrap();

        let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy.port()))
            .await
            .unwrap();
        let initial_body = format!("prefix_{}_krun-secret:API_KEY", "A".repeat(5000));
        let req = format!(
            "POST http://127.0.0.1:{}/test HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\n\r\n{}",
            upstream_addr.port(),
            upstream_addr.port(),
            initial_body.len(),
            initial_body
        );

        client.write_all(req.as_bytes()).await.unwrap();

        let mut resp = [0u8; 256];
        let n = client.read(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp[..n]);
        assert!(resp_str.contains("200 OK"));
    }
}
