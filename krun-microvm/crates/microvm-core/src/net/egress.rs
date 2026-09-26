// Copyright 2026, libkrun-sdk authors.
// SPDX-License-Identifier: Apache-2.0

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

/// Egress policy defining permitted outbound network destinations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// Permitted destinations in `host:port`, `host`, `CIDR` (e.g. `10.0.0.0/8`),
    /// or wildcard formats (e.g. `["api.openai.com:443", "*.github.com:443", "*:443", "192.168.1.0/24"]`).
    pub allow_hosts: Vec<String>,
}

impl EgressPolicy {
    pub fn new(allow_hosts: Vec<String>) -> Self {
        Self { allow_hosts }
    }

    /// Determines whether an outbound request to `(target_host, target_port)` is permitted.
    ///
    /// Rules:
    /// 1. Cloud metadata endpoints (AWS, GCP, Azure, Alibaba, OpenStack) are strictly denied
    ///    unless explicitly allowed by a specific matching rule.
    /// 2. If `allow_hosts` is empty, egress is unconstrained (standard container networking),
    ///    except cloud metadata which remains strictly blocked.
    /// 3. If `allow_hosts` is non-empty, all unlisted destinations are denied (default-deny egress).
    pub fn is_allowed(&self, target_host: &str, target_port: u16) -> bool {
        // Strict guard against cloud instance metadata exfiltration across all major clouds
        if is_cloud_metadata(target_host) {
            return self.matches_any(target_host, target_port);
        }

        if self.allow_hosts.is_empty() {
            return true;
        }

        self.matches_any(target_host, target_port)
    }

    /// Checks if a resolved IP address was explicitly permitted in the allowlist.
    /// Used for DNS rebinding protection against exfiltration to cloud metadata or link-local IPs.
    pub fn is_ip_explicitly_allowed(&self, ip: IpAddr, port: u16) -> bool {
        let ip_str = ip.to_string();
        self.matches_any(&ip_str, port)
    }

    pub fn matches_any(&self, target_host: &str, target_port: u16) -> bool {
        let target_clean = target_host.trim().trim_matches(|c| c == '[' || c == ']');
        let target_ip_opt = target_clean.parse::<IpAddr>().ok();

        for rule in &self.allow_hosts {
            let rule = rule.trim();
            if rule == "*" || rule == "*:*" {
                return true;
            }

            // Check if rule is CIDR: e.g. 10.0.0.0/8 or 10.0.0.0/8:8080
            if rule.contains('/') {
                let (cidr_part, rule_port) = if let Some((c, p_str)) = rule.rsplit_once(':') {
                    if let Ok(p) = p_str.parse::<u16>() {
                        (c, Some(p))
                    } else if p_str == "*" {
                        (c, None)
                    } else {
                        (rule, None)
                    }
                } else {
                    (rule, None)
                };

                if let Some(ref target_ip) = target_ip_opt {
                    if ip_in_cidr(target_ip, cidr_part) {
                        if let Some(p) = rule_port {
                            if p == target_port {
                                return true;
                            }
                        } else {
                            return true;
                        }
                    }
                }
                continue;
            }

            // Rule in host:port or [ipv6]:port format
            if let Ok((rule_host, rule_port)) = parse_host_port(rule, 0) {
                if rule_port != 0 {
                    if rule_port == target_port && Self::host_matches(&rule_host, target_clean) {
                        return true;
                    }
                } else if Self::host_matches(rule, target_clean) {
                    return true;
                }
            } else if Self::host_matches(rule, target_clean) {
                return true;
            }
        }
        false
    }

    pub fn host_matches(pattern: &str, host: &str) -> bool {
        let pattern = pattern
            .trim()
            .trim_matches(|c| c == '[' || c == ']')
            .to_lowercase();
        let host = host
            .trim()
            .trim_matches(|c| c == '[' || c == ']')
            .to_lowercase();

        if pattern == "*" || pattern == host {
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

/// Matches an IP address against a CIDR prefix string (e.g. `10.0.0.0/8` or `fd00::/8`).
pub fn ip_in_cidr(ip: &IpAddr, cidr_str: &str) -> bool {
    let (net_str, prefix_str) = match cidr_str.split_once('/') {
        Some(parts) => parts,
        None => return false,
    };
    let prefix_len: u32 = match prefix_str.parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    match (ip, net_str.parse::<IpAddr>()) {
        (IpAddr::V4(target_v4), Ok(IpAddr::V4(net_v4))) => {
            if prefix_len > 32 {
                return false;
            }
            if prefix_len == 0 {
                return true;
            }
            let mask = !((1u32 << (32 - prefix_len)) - 1);
            let target_u32 = u32::from(*target_v4);
            let net_u32 = u32::from(net_v4);
            (target_u32 & mask) == (net_u32 & mask)
        }
        (IpAddr::V6(target_v6), Ok(IpAddr::V6(net_v6))) => {
            if prefix_len > 128 {
                return false;
            }
            if prefix_len == 0 {
                return true;
            }
            let mask = !((1u128 << (128 - prefix_len)) - 1);
            let target_u128 = u128::from(*target_v6);
            let net_u128 = u128::from(net_v6);
            (target_u128 & mask) == (net_u128 & mask)
        }
        _ => false,
    }
}

/// Detects cloud instance metadata endpoints across AWS, Azure, GCP, Alibaba, OpenStack, and Oracle.
pub fn is_cloud_metadata(target_host: &str) -> bool {
    let host_clean = target_host.trim().trim_matches(|c| c == '[' || c == ']');
    let host_lower = host_clean.to_lowercase();

    // Exact string checks for cloud metadata DNS hostnames
    if host_lower == "metadata.google.internal"
        || host_lower.ends_with(".metadata.google.internal")
        || host_lower == "instance-data"
        || host_lower.ends_with(".instance-data")
    {
        return true;
    }

    // Direct IPv4 link-local (169.254.0.0/16) prefix or specific IP
    if host_lower == "169.254.169.254"
        || host_lower.starts_with("169.254.")
        || host_lower == "100.100.100.200" // Alibaba Cloud IMDS
    {
        return true;
    }

    // Direct IPv6 link-local (fe80::/10) or AWS IMDSv2 (fd00:ec2::254)
    if host_lower == "fd00:ec2::254"
        || host_lower.starts_with("fe80:")
        || host_lower.starts_with("fe80::")
    {
        return true;
    }

    // Parse as IP address if possible
    if let Ok(ip) = host_clean.parse::<IpAddr>() {
        return is_cloud_metadata_ip(&ip);
    }

    // Decimal encoding of 169.254.169.254 = 2852039166
    if let Ok(num) = host_clean.parse::<u32>() {
        let octets = num.to_be_bytes();
        if (octets[0] == 169 && octets[1] == 254) || octets == [100, 100, 100, 200] {
            return true;
        }
    }

    false
}

/// Verifies whether an IP address belongs to cloud metadata or link-local address spaces.
pub fn is_cloud_metadata_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 169.254.0.0/16 (Link Local / IMDS)
            if octets[0] == 169 && octets[1] == 254 {
                return true;
            }
            // Alibaba Cloud IMDS: 100.100.100.200
            if octets == [100, 100, 100, 200] {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            // fe80::/10 link-local
            let segments = v6.segments();
            if (segments[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // fd00:ec2::254 (AWS EC2 IPv6 IMDSv2)
            if segments == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254] {
                return true;
            }
            // IPv4-mapped IPv6 (::ffff:169.254.x.x)
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_cloud_metadata_ip(&IpAddr::V4(v4));
            }
            false
        }
    }
}

/// Zero-trust secret substitution engine.
///
/// Replaces placeholder tokens (e.g. `krun-secret:OPENAI_API_KEY` and URL-encoded `krun-secret%3A...`)
/// with real credentials in-flight during outbound proxy requests.
#[derive(Debug, Clone, Default)]
pub struct SecretSubstitution {
    mapping: Vec<(Vec<u8>, Vec<u8>)>,
    raw_keys: Vec<String>,
}

impl SecretSubstitution {
    pub fn new(secrets: &[(String, String)]) -> Self {
        let mut mapping = Vec::new();
        let mut raw_keys = Vec::new();
        for (key, val) in secrets {
            raw_keys.push(key.clone());
            // Raw placeholder: krun-secret:KEY
            let raw_ph = format!("krun-secret:{}", key);
            mapping.push((raw_ph.into_bytes(), val.as_bytes().to_vec()));

            // URL-encoded placeholder (%3A and %3a)
            let url_ph_upper = format!("krun-secret%3A{}", key);
            mapping.push((url_ph_upper.into_bytes(), val.as_bytes().to_vec()));

            let url_ph_lower = format!("krun-secret%3a{}", key);
            mapping.push((url_ph_lower.into_bytes(), val.as_bytes().to_vec()));
        }
        Self { mapping, raw_keys }
    }

    /// Replaces placeholder occurrences inside an HTTP request buffer.
    /// Binary-safe: does not fail or abort on non-UTF-8 payload chunks.
    pub fn substitute(&self, buffer: &[u8]) -> Vec<u8> {
        if self.mapping.is_empty() || buffer.is_empty() {
            return buffer.to_vec();
        }

        let mut current = buffer.to_vec();
        for (needle, replacement) in &self.mapping {
            if needle.is_empty() {
                continue;
            }
            current = replace_bytes(&current, needle, replacement);
        }
        current
    }

    /// Replaces placeholder occurrences in a text string.
    pub fn substitute_str(&self, text: &str) -> String {
        let sub = self.substitute(text.as_bytes());
        String::from_utf8_lossy(&sub).to_string()
    }

    pub fn has_secrets(&self) -> bool {
        !self.mapping.is_empty()
    }

    pub fn keys(&self) -> &[String] {
        &self.raw_keys
    }
}

fn replace_bytes(src: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() || src.len() < needle.len() {
        return src.to_vec();
    }
    let mut result = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(needle) {
            result.extend_from_slice(replacement);
            i += needle.len();
        } else {
            result.push(src[i]);
            i += 1;
        }
    }
    result
}

/// LLM Token Metering and Hard Budget Enforcer.
/// Supports OpenAI, Anthropic, Google Gemini/Vertex, Mistral, Ollama, Groq, and Cohere.
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
    /// to extract and count token usage across all major providers.
    pub fn inspect_chunk(&self, chunk: &[u8]) {
        let text = match std::str::from_utf8(chunk) {
            Ok(t) => t,
            Err(_) => return,
        };

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
                self.extract_tokens_from_value(&val);
            }
        }
    }

    fn extract_tokens_from_value(&self, val: &serde_json::Value) {
        // 1. OpenAI / standard format: "usage": { "total_tokens": N }
        // or Anthropic: "usage": { "input_tokens": A, "output_tokens": B }
        if let Some(usage) = val.get("usage") {
            if let Some(total) = usage.get("total_tokens").and_then(|t| t.as_u64()) {
                self.record_tokens(total);
                return;
            }
            let input = usage.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            let output = usage.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            if input + output > 0 {
                self.record_tokens(input + output);
                return;
            }
            let prompt = usage.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            let comp = usage.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            if prompt + comp > 0 {
                self.record_tokens(prompt + comp);
                return;
            }
        }

        // 2. Google Gemini / Vertex format: "usageMetadata": { "totalTokenCount": N }
        if let Some(usage) = val.get("usageMetadata") {
            if let Some(total) = usage.get("totalTokenCount").and_then(|t| t.as_u64()) {
                self.record_tokens(total);
                return;
            }
            let prompt = usage.get("promptTokenCount").and_then(|t| t.as_u64()).unwrap_or(0);
            let candidates = usage.get("candidatesTokenCount").and_then(|t| t.as_u64()).unwrap_or(0);
            if prompt + candidates > 0 {
                self.record_tokens(prompt + candidates);
                return;
            }
        }

        // 3. Ollama format: "prompt_eval_count": N, "eval_count": M
        let prompt_eval = val.get("prompt_eval_count").and_then(|t| t.as_u64()).unwrap_or(0);
        let eval = val.get("eval_count").and_then(|t| t.as_u64()).unwrap_or(0);
        if prompt_eval + eval > 0 {
            self.record_tokens(prompt_eval + eval);
            return;
        }

        // 4. Cohere format: "meta": { "tokens": { "input_tokens": A, "output_tokens": B } }
        if let Some(meta) = val.get("meta") {
            if let Some(tokens) = meta.get("tokens") {
                let input = tokens.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                let output = tokens.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                if input + output > 0 {
                    self.record_tokens(input + output);
                    return;
                }
            }
        }

        // 5. Direct top-level total_tokens or tokens
        if let Some(total) = val.get("total_tokens").and_then(|t| t.as_u64()) {
            self.record_tokens(total);
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

pub fn find_header_end(buf: &[u8]) -> Option<(usize, usize)> {
    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some((pos, pos + 4))
    } else {
        buf.windows(2)
            .position(|w| w == b"\n\n")
            .map(|pos| (pos, pos + 2))
    }
}

pub fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        if line.to_ascii_lowercase().starts_with("content-length:") {
            if let Some((_, val)) = line.split_once(':') {
                return val.trim().parse::<usize>().ok();
            }
        }
    }
    None
}

pub fn adjust_content_length(headers: &str, new_body_len: usize) -> String {
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

/// Sanitizes outbound forward proxy request headers:
/// 1. Rewrites request line from absolute URI (GET http://host/path) to origin-form (GET /path HTTP/1.1)
/// 2. Injects Connection: close to avoid socket hanging on upstream keep-alive
/// 3. Normalizes Host header
/// 4. Strips proxy-only headers (Proxy-Connection, Proxy-Authorization)
/// 5. Adjusts Content-Length
pub fn sanitize_and_rewrite_request(
    headers_raw: &[u8],
    method: &str,
    path: &str,
    target_host: &str,
    target_port: u16,
    new_body_len: Option<usize>,
) -> Vec<u8> {
    let headers_str = String::from_utf8_lossy(headers_raw);
    let mut lines = headers_str.lines();
    let _first_line = lines.next(); // discard original line

    let host_val = if target_port == 80 || target_port == 443 {
        target_host.to_string()
    } else if target_host.contains(':') && !target_host.starts_with('[') {
        format!("[{}]:{}", target_host, target_port)
    } else {
        format!("{}:{}", target_host, target_port)
    };

    let mut updated_lines = Vec::new();
    let origin_path = if path.is_empty() { "/" } else { path };
    updated_lines.push(format!("{} {} HTTP/1.1", method, origin_path));

    let mut has_host = false;
    let mut has_conn = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();

        if lower.starts_with("proxy-connection:") || lower.starts_with("proxy-authorization:") {
            continue;
        }

        if lower.starts_with("host:") {
            updated_lines.push(format!("Host: {}", host_val));
            has_host = true;
            continue;
        }

        if lower.starts_with("content-length:") {
            if let Some(new_len) = new_body_len {
                updated_lines.push(format!("Content-Length: {}", new_len));
            } else {
                updated_lines.push(line.to_string());
            }
            continue;
        }

        if lower.starts_with("connection:") {
            updated_lines.push("Connection: close".to_string());
            has_conn = true;
            continue;
        }

        updated_lines.push(line.to_string());
    }

    if !has_host {
        updated_lines.push(format!("Host: {}", host_val));
    }
    if !has_conn {
        updated_lines.push("Connection: close".to_string());
    }
    if let Some(new_len) = new_body_len {
        if !updated_lines.iter().any(|l| l.to_ascii_lowercase().starts_with("content-length:")) {
            updated_lines.push(format!("Content-Length: {}", new_len));
        }
    }

    let mut out = updated_lines.join("\r\n");
    out.push_str("\r\n\r\n");
    out.into_bytes()
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

    let request_head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let first_line = request_head.lines().next().unwrap_or_default();
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("Malformed HTTP request from client");
    }

    let method = parts[0].to_string();
    let target = parts[1].to_string();

    let host_header = request_head.lines().find_map(|line| {
        if line.to_ascii_lowercase().starts_with("host:") {
            line.split_once(':').map(|(_, v)| v.trim().to_string())
        } else {
            None
        }
    });

    if method.eq_ignore_ascii_case("CONNECT") {
        // HTTPS tunneling: CONNECT target:port HTTP/1.1
        let (host, port) = parse_host_port(&target, 443)?;

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

        // DNS Rebinding Defense: resolve upstream addresses and verify none are cloud metadata
        let addrs: Vec<SocketAddr> = match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(iter) => iter.collect(),
            Err(e) => {
                let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 22\r\n\r\nDNS resolution failed\r\n";
                let _ = client.write_all(resp).await;
                bail!("DNS resolution failed for {}:{}: {}", host, port, e);
            }
        };

        if addrs.is_empty() {
            let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 20\r\n\r\nNo IP address found\r\n";
            let _ = client.write_all(resp).await;
            bail!("No IP addresses found for {}:{}", host, port);
        }

        // Verify resolved IPs against cloud metadata
        for addr in &addrs {
            let ip = addr.ip();
            if is_cloud_metadata_ip(&ip) && !policy.is_ip_explicitly_allowed(ip, port) {
                blocked_requests.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("DNS rebinding attempt to metadata IP {ip} BLOCKED for target {host}:{port}");
                let resp = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 43\r\n\r\nCloud metadata exfiltration defense triggered\r\n";
                client.write_all(resp).await?;
                return Ok(());
            }
        }

        let mut upstream = match TcpStream::connect(addrs.as_slice()).await {
            Ok(s) => s,
            Err(e) => {
                let resp = b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 26\r\n\r\nUpstream connection failed\r\n";
                let _ = client.write_all(resp).await;
                bail!("Failed to connect to target {}:{}: {}", host, port, e);
            }
        };

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
        let (host, port, path) = parse_http_target(&target, host_header.as_deref())?;

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

        // DNS Rebinding Defense: resolve upstream and check for cloud metadata
        let addrs: Vec<SocketAddr> = match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(iter) => iter.collect(),
            Err(e) => {
                let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 22\r\n\r\nDNS resolution failed\r\n";
                let _ = client.write_all(resp).await;
                bail!("DNS resolution failed for {}:{}: {}", host, port, e);
            }
        };

        if addrs.is_empty() {
            let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 20\r\n\r\nNo IP address found\r\n";
            let _ = client.write_all(resp).await;
            bail!("No IP addresses found for {}:{}", host, port);
        }

        for addr in &addrs {
            let ip = addr.ip();
            if is_cloud_metadata_ip(&ip) && !policy.is_ip_explicitly_allowed(ip, port) {
                blocked_requests.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("DNS rebinding attempt to metadata IP {ip} BLOCKED for target {host}:{port}");
                let resp = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 43\r\n\r\nCloud metadata exfiltration defense triggered\r\n";
                client.write_all(resp).await?;
                return Ok(());
            }
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

        // Sanitize and rewrite the request headers:
        // 1. Rewrite first line from absolute URI (e.g. GET http://host/path HTTP/1.1) to origin-form (GET /path HTTP/1.1)
        // 2. Adjust Content-Length to match substituted_body.len()
        // 3. Ensure Connection: close is set so upstream closes gracefully
        let rewritten_headers = sanitize_and_rewrite_request(
            &substituted_headers,
            &method,
            &path,
            &host,
            port,
            if content_len.is_some() { Some(substituted_body.len()) } else { None },
        );

        let mut final_req = rewritten_headers;
        final_req.extend_from_slice(&substituted_body);

        let mut upstream = match TcpStream::connect(addrs.as_slice()).await {
            Ok(s) => s,
            Err(e) => {
                let resp = b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 26\r\n\r\nUpstream connection failed\r\n";
                let _ = client.write_all(resp).await;
                bail!("Failed to connect to HTTP target {}:{}: {}", host, port, e);
            }
        };

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

pub fn parse_host_port(target: &str, default_port: u16) -> Result<(String, u16)> {
    let target = target.trim();
    if target.is_empty() {
        bail!("Empty target host:port");
    }

    if target.starts_with('[') {
        if let Some(close_bracket) = target.find(']') {
            let host = target[1..close_bracket].to_string();
            let remainder = &target[close_bracket + 1..];
            let port = if let Some(port_str) = remainder.strip_prefix(':') {
                port_str.parse::<u16>().unwrap_or(default_port)
            } else {
                default_port
            };
            return Ok((host, port));
        }
    }

    // Check if target is an unbracketed IPv6 with multiple colons
    let colon_count = target.chars().filter(|c| *c == ':').count();
    if colon_count > 1 {
        return Ok((target.to_string(), default_port));
    }

    if let Some((h, p)) = target.split_once(':') {
        let port = p.parse::<u16>().unwrap_or(default_port);
        Ok((h.to_string(), port))
    } else {
        Ok((target.to_string(), default_port))
    }
}

pub fn parse_http_target(target: &str, host_header: Option<&str>) -> Result<(String, u16, String)> {
    if let Some(rest) = target.strip_prefix("http://") {
        let (host_part, path) = if let Some(slash_pos) = rest.find('/') {
            (&rest[..slash_pos], rest[slash_pos..].to_string())
        } else {
            (rest, "/".to_string())
        };
        let (host, port) = parse_host_port(host_part, 80)?;
        Ok((host, port, path))
    } else if let Some(rest) = target.strip_prefix("https://") {
        let (host_part, path) = if let Some(slash_pos) = rest.find('/') {
            (&rest[..slash_pos], rest[slash_pos..].to_string())
        } else {
            (rest, "/".to_string())
        };
        let (host, port) = parse_host_port(host_part, 443)?;
        Ok((host, port, path))
    } else if target.starts_with('/') {
        let host_str = host_header.unwrap_or("127.0.0.1");
        let (host, port) = parse_host_port(host_str, 80)?;
        Ok((host, port, target.to_string()))
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
        assert!(!open_policy.is_allowed("metadata.google.internal", 80));
        assert!(!open_policy.is_allowed("fd00:ec2::254", 80));
        assert!(!open_policy.is_allowed("[fd00:ec2::254]", 80));
        assert!(!open_policy.is_allowed("100.100.100.200", 80));
        assert!(!open_policy.is_allowed("2852039166", 80)); // decimal 169.254.169.254

        // Only allowed if explicitly in rules
        let explicit_policy = EgressPolicy::new(vec!["169.254.169.254:80".to_string()]);
        assert!(explicit_policy.is_allowed("169.254.169.254", 80));
    }

    #[test]
    fn test_egress_policy_cidr_matching() {
        let policy = EgressPolicy::new(vec![
            "10.0.0.0/8:8080".to_string(),
            "192.168.1.0/24".to_string(),
        ]);

        assert!(policy.is_allowed("10.1.2.3", 8080));
        assert!(!policy.is_allowed("10.1.2.3", 80)); // wrong port for 10.0.0.0/8
        assert!(policy.is_allowed("192.168.1.50", 443)); // any port allowed
        assert!(policy.is_allowed("192.168.1.1", 80));
        assert!(!policy.is_allowed("192.168.2.1", 80));
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
    fn test_secret_substitution_url_encoded() {
        let secrets = vec![("API_KEY".to_string(), "my-real-secret-999".to_string())];
        let engine = SecretSubstitution::new(&secrets);

        let url_query = b"GET /v1/models?token=krun-secret%3AAPI_KEY&other=krun-secret%3aAPI_KEY HTTP/1.1\r\n\r\n";
        let out = engine.substitute(url_query);
        let out_str = String::from_utf8(out).unwrap();
        assert!(out_str.contains("token=my-real-secret-999"));
        assert!(out_str.contains("other=my-real-secret-999"));
        assert!(!out_str.contains("krun-secret"));
    }

    #[test]
    fn test_llm_token_budget_enforcement() {
        let budget = LlmTokenBudget::new(Some(100));
        assert!(!budget.is_exceeded());

        // Parse standard json usage (OpenAI)
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

    #[test]
    fn test_multi_provider_llm_token_budget() {
        // Test Anthropic format: input_tokens + output_tokens
        let anthropic_budget = LlmTokenBudget::new(None);
        let anthropic_chunk = br#"{"type":"message","usage":{"input_tokens":40,"output_tokens":60}}"#;
        anthropic_budget.inspect_chunk(anthropic_chunk);
        assert_eq!(anthropic_budget.total_consumed(), 100);

        // Test Google Gemini format: usageMetadata
        let gemini_budget = LlmTokenBudget::new(None);
        let gemini_chunk = br#"{"candidates":[{"content":"hi"}],"usageMetadata":{"promptTokenCount":15,"candidatesTokenCount":35,"totalTokenCount":50}}"#;
        gemini_budget.inspect_chunk(gemini_chunk);
        assert_eq!(gemini_budget.total_consumed(), 50);

        // Test Ollama format: prompt_eval_count + eval_count
        let ollama_budget = LlmTokenBudget::new(None);
        let ollama_chunk = br#"{"model":"llama3","prompt_eval_count":12,"eval_count":28}"#;
        ollama_budget.inspect_chunk(ollama_chunk);
        assert_eq!(ollama_budget.total_consumed(), 40);
    }

    #[test]
    fn test_parse_host_port_ipv4_and_ipv6() {
        // Standard IPv4
        let (h, p) = parse_host_port("api.openai.com:443", 80).unwrap();
        assert_eq!(h, "api.openai.com");
        assert_eq!(p, 443);

        // IPv4 with default port
        let (h, p) = parse_host_port("api.openai.com", 443).unwrap();
        assert_eq!(h, "api.openai.com");
        assert_eq!(p, 443);

        // Bracketed IPv6 with port
        let (h, p) = parse_host_port("[2001:db8::1]:8080", 80).unwrap();
        assert_eq!(h, "2001:db8::1");
        assert_eq!(p, 8080);

        // Bracketed IPv6 without port
        let (h, p) = parse_host_port("[fe80::1]", 443).unwrap();
        assert_eq!(h, "fe80::1");
        assert_eq!(p, 443);

        // Raw IPv6 with multiple colons
        let (h, p) = parse_host_port("2001:db8::1", 443).unwrap();
        assert_eq!(h, "2001:db8::1");
        assert_eq!(p, 443);
    }

    #[test]
    fn test_parse_http_target() {
        // Absolute URL
        let (h, p, path) = parse_http_target("http://api.openai.com:8080/v1/models", None).unwrap();
        assert_eq!(h, "api.openai.com");
        assert_eq!(p, 8080);
        assert_eq!(path, "/v1/models");

        // Origin-form path with Host header
        let (h, p, path) = parse_http_target("/v1/chat", Some("api.anthropic.com:443")).unwrap();
        assert_eq!(h, "api.anthropic.com");
        assert_eq!(p, 443);
        assert_eq!(path, "/v1/chat");
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
