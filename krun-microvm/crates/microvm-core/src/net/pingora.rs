// Copyright 2026, libkrun-sdk authors.
// SPDX-License-Identifier: Apache-2.0

//! Cloudflare Pingora integration for high-performance L4/L7 Ingress, Reverse Proxying,
//! and Zero-Trust Egress filtering in `libkrun-sdk`.

use crate::net::egress::{parse_host_port, EgressPolicy, SecretSubstitution};
use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::proxy::{http_proxy_service, HttpProxy, ProxyHttp, Session};
use pingora::server::configuration::ServerConf;
use pingora::server::Server;
use pingora::services::listening::Service;
use pingora::upstreams::peer::HttpPeer;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Session context tracked for each in-flight request through Pingora.
#[derive(Default, Debug, Clone)]
pub struct PingoraRequestContext {
    pub target_host: String,
    pub target_port: u16,
    pub is_tls: bool,
    pub blocked: bool,
    pub tokens_counted: u64,
}

/// Zero-Trust Egress Proxy implemented as a Cloudflare Pingora HTTP/HTTPS proxy.
///
/// Features:
/// 1. Default-deny domain whitelisting with wildcard (`*.github.com`) and CIDR support.
/// 2. Strict cloud metadata defense (blocks SSRF to `169.254.169.254`, `100.100.100.200`, `fd00:ec2::254`).
/// 3. In-flight secret substitution in headers, URI queries, and request body chunks.
/// 4. Upstream connection pooling with HTTP/1.1 and HTTP/2 multiplexing.
/// 5. Streaming LLM token ceiling budget enforcement.
pub struct PingoraEgressProxy {
    pub policy: Arc<EgressPolicy>,
    pub secret_substitution: Arc<SecretSubstitution>,
    pub token_meter: Option<Arc<AtomicU64>>,
    pub token_ceiling: Option<u64>,
}

impl PingoraEgressProxy {
    pub fn new(
        policy: EgressPolicy,
        secrets: &[(String, String)],
        max_tokens: Option<u64>,
    ) -> Self {
        Self {
            policy: Arc::new(policy),
            secret_substitution: Arc::new(SecretSubstitution::new(secrets)),
            token_meter: max_tokens.map(|_| Arc::new(AtomicU64::new(0))),
            token_ceiling: max_tokens,
        }
    }

    pub fn tokens_consumed(&self) -> u64 {
        self.token_meter
            .as_ref()
            .map(|m| m.load(Ordering::Relaxed))
            .unwrap_or(0)
    }
}

#[async_trait]
impl ProxyHttp for PingoraEgressProxy {
    type CTX = PingoraRequestContext;

    fn new_ctx(&self) -> Self::CTX {
        PingoraRequestContext::default()
    }

    /// Fast-path early request inspection and policy verification.
    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<bool> {
        let req_header = session.req_header();

        // Extract target destination from URI or Host header
        let host = req_header
            .headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .or_else(|| req_header.uri.host())
            .unwrap_or("unknown");

        let is_https = req_header.uri.scheme_str() == Some("https");
        let default_port = if is_https { 443 } else { 80 };

        let (target_host, target_port) = match parse_host_port(host, default_port) {
            Ok((h, p)) => (h, p),
            Err(_) => (host.to_string(), default_port),
        };

        ctx.target_host = target_host.clone();
        ctx.target_port = target_port;
        ctx.is_tls = target_port == 443 || is_https;

        // Verify egress policy (domain whitelist, CIDR & cloud metadata protection)
        if !self.policy.is_allowed(&target_host, target_port) {
            ctx.blocked = true;
            tracing::warn!(
                "[PingoraEgress] Blocked unauthorized outbound egress: {}:{}",
                target_host,
                target_port
            );

            // Construct Pingora 403 Forbidden downstream response
            let mut resp = ResponseHeader::build(403, None)?;
            resp.append_header("Content-Type", "application/json")?;
            resp.append_header("X-Proxy-Engine", "Cloudflare-Pingora-libkrun")?;
            let body = format!(
                r#"{{"error":"EgressBlocked","message":"Destination '{}:{}' denied by zero-trust egress policy"}}"#,
                target_host, target_port
            );
            session.write_response_header(Box::new(resp), false).await?;
            session
                .write_response_body(Some(Bytes::from(body)), true)
                .await?;

            // Return true to terminate the request processing in Pingora
            return Ok(true);
        }

        Ok(false)
    }

    /// Resolves the upstream peer connection with connection pooling and TLS.
    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<Box<HttpPeer>> {
        let peer_addr = if ctx.target_host.contains(':') && !ctx.target_host.starts_with('[') {
            format!("[{}]:{}", ctx.target_host, ctx.target_port)
        } else {
            format!("{}:{}", ctx.target_host, ctx.target_port)
        };

        let mut peer = Box::new(HttpPeer::new(
            peer_addr,
            ctx.is_tls,
            ctx.target_host.clone(),
        ));

        // Configure connection timeouts and pooling options
        peer.options.connection_timeout = Some(Duration::from_secs(5));
        peer.options.read_timeout = Some(Duration::from_secs(60));
        peer.options.idle_timeout = Some(Duration::from_secs(60));

        Ok(peer)
    }

    /// Rewrites outbound request headers and URI query for in-flight secret substitution.
    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        // 1. Substitute in URI (path and query parameters, e.g. ?key=krun-secret:API_KEY)
        let uri_str = upstream_request.uri.to_string();
        if uri_str.contains("krun-secret:") || uri_str.contains("krun-secret%3A") || uri_str.contains("krun-secret%3a") {
            let substituted = self.secret_substitution.substitute(uri_str.as_bytes());
            if let Ok(new_uri_str) = std::str::from_utf8(&substituted) {
                if let Ok(new_uri) = new_uri_str.parse::<http::Uri>() {
                    upstream_request.set_uri(new_uri);
                }
            }
        }

        // 2. Substitute in HTTP request headers
        let mut updates = Vec::new();
        for (name, value) in upstream_request.headers.iter() {
            if let Ok(val_str) = value.to_str() {
                if val_str.contains("krun-secret:") || val_str.contains("krun-secret%3A") || val_str.contains("krun-secret%3a") {
                    let substituted = self.secret_substitution.substitute(val_str.as_bytes());
                    if let Ok(new_val) = std::str::from_utf8(&substituted) {
                        updates.push((name.clone(), new_val.to_string()));
                    }
                }
            }
        }
        for (name, new_val) in updates {
            let _ = upstream_request.insert_header(name, new_val);
        }
        Ok(())
    }

    /// Rewrites outbound request body chunks for in-flight secret substitution.
    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        _end_of_stream: bool,
        _ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        if let Some(ref chunk) = body {
            let substituted = self.secret_substitution.substitute(chunk);
            *body = Some(Bytes::from(substituted));
        }
        Ok(())
    }

    /// Response body streaming filter to track LLM token expenditure.
    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        _end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<Option<Duration>> {
        if let Some(ref chunk) = body {
            if let Some(ref meter) = self.token_meter {
                // Approximate 4 chars per token for streaming JSON / SSE
                let tokens = (chunk.len() as u64) / 4 + 1;
                let current = meter.fetch_add(tokens, Ordering::Relaxed) + tokens;
                ctx.tokens_counted += tokens;

                if let Some(ceiling) = self.token_ceiling {
                    if current > ceiling {
                        tracing::error!(
                            "[PingoraEgress] Hard LLM token ceiling exceeded ({} > {}). Dropping stream.",
                            current, ceiling
                        );
                        // Truncate response chunk to terminate downstream stream
                        *body = Some(Bytes::from_static(
                            b"\n[ERROR: KRUN_LLM_TOKEN_BUDGET_EXCEEDED]\n",
                        ));
                    }
                }
            }
        }
        Ok(None)
    }
}

/// Upstream destination for MicroVM service routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroVmServiceBackend {
    pub service_name: String,
    pub target_addr: SocketAddr,
    pub path_prefix: String,
    pub strip_prefix: bool,
}

impl MicroVmServiceBackend {
    pub fn new(service_name: impl Into<String>, target_addr: SocketAddr, path_prefix: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            target_addr,
            path_prefix: path_prefix.into(),
            strip_prefix: false,
        }
    }

    pub fn with_strip_prefix(mut self, strip: bool) -> Self {
        self.strip_prefix = strip;
        self
    }
}

/// Dynamic Ingress and Reverse Proxy Gateway for MicroVMs and Compose Services.
pub struct PingoraMicroVmGateway {
    pub routes: Arc<HashMap<String, MicroVmServiceBackend>>,
}

impl PingoraMicroVmGateway {
    pub fn new(routes: HashMap<String, MicroVmServiceBackend>) -> Self {
        Self {
            routes: Arc::new(routes),
        }
    }
}

#[async_trait]
impl ProxyHttp for PingoraMicroVmGateway {
    type CTX = Option<MicroVmServiceBackend>;

    fn new_ctx(&self) -> Self::CTX {
        None
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<bool> {
        let path = session.req_header().uri.path();

        // Built-in Health check endpoint for ingress gateway
        if path == "/healthz" || path == "/_health" || path == "/_gateway/health" {
            let mut resp = ResponseHeader::build(200, None)?;
            resp.append_header("Content-Type", "application/json")?;
            resp.append_header("X-Gateway", "Pingora-libkrun-microvm")?;
            let body = format!(
                r#"{{"status":"ok","engine":"Cloudflare-Pingora","routes_count":{}}}"#,
                self.routes.len()
            );
            session.write_response_header(Box::new(resp), false).await?;
            session
                .write_response_body(Some(Bytes::from(body)), true)
                .await?;
            return Ok(true);
        }

        // Route matching by longest prefix
        let mut matched: Option<&MicroVmServiceBackend> = None;
        for (prefix, backend) in self.routes.iter() {
            let normalized_prefix = prefix.trim_end_matches('/');
            if path == normalized_prefix
                || path.starts_with(&format!("{}/", normalized_prefix))
                || path.starts_with(prefix)
            {
                if matched.is_none() || prefix.len() > matched.unwrap().path_prefix.len() {
                    matched = Some(backend);
                }
            }
        }

        if let Some(backend) = matched {
            *ctx = Some(backend.clone());
            Ok(false)
        } else {
            // No matching route
            let mut resp = ResponseHeader::build(404, None)?;
            resp.append_header("Content-Type", "application/json")?;
            resp.append_header("X-Gateway", "Pingora-libkrun-microvm")?;
            let body =
                r#"{"error":"NotFound","message":"No microVM service route matched request path"}"#;
            session.write_response_header(Box::new(resp), false).await?;
            session
                .write_response_body(Some(Bytes::from(body)), true)
                .await?;
            Ok(true)
        }
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        if let Some(backend) = ctx.as_ref() {
            if backend.strip_prefix {
                let current_path = upstream_request.uri.path();
                let new_path = if let Some(stripped) = current_path.strip_prefix(&backend.path_prefix) {
                    if stripped.is_empty() {
                        "/".to_string()
                    } else if !stripped.starts_with('/') {
                        format!("/{}", stripped)
                    } else {
                        stripped.to_string()
                    }
                } else {
                    current_path.to_string()
                };

                let query_str = upstream_request
                    .uri
                    .query()
                    .map(|q| format!("?{}", q))
                    .unwrap_or_default();
                let full_uri = format!("{}{}", new_path, query_str);
                if let Ok(uri) = full_uri.parse::<http::Uri>() {
                    upstream_request.set_uri(uri);
                }
            }
        }
        Ok(())
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<Box<HttpPeer>> {
        let backend = ctx.as_ref().ok_or_else(|| {
            pingora::Error::explain(pingora::ErrorType::HTTPStatus(404), "No upstream route")
        })?;

        let peer = Box::new(HttpPeer::new(
            backend.target_addr,
            false,
            backend.service_name.clone(),
        ));
        Ok(peer)
    }
}

/// Helper to create a ready-to-run Pingora Service on a given listening address.
pub fn create_pingora_egress_service(
    listen_addr: &str,
    policy: EgressPolicy,
    secrets: &[(String, String)],
    max_tokens: Option<u64>,
) -> Result<Service<HttpProxy<PingoraEgressProxy>>> {
    let app = PingoraEgressProxy::new(policy, secrets, max_tokens);
    let conf = Arc::new(ServerConf::default());
    let mut service = http_proxy_service(&conf, app);
    service.add_tcp(listen_addr);
    Ok(service)
}

/// Helper to create a ready-to-run Pingora Ingress Gateway Service.
pub fn create_pingora_gateway_service(
    listen_addr: &str,
    routes: HashMap<String, MicroVmServiceBackend>,
) -> Result<Service<HttpProxy<PingoraMicroVmGateway>>> {
    let app = PingoraMicroVmGateway::new(routes);
    let conf = Arc::new(ServerConf::default());
    let mut service = http_proxy_service(&conf, app);
    service.add_tcp(listen_addr);
    Ok(service)
}

/// Runs the Pingora egress proxy server synchronously (blocks current thread).
pub fn run_pingora_egress_server(
    listen_addr: &str,
    policy: EgressPolicy,
    secrets: &[(String, String)],
    max_tokens: Option<u64>,
) -> Result<()> {
    let service = create_pingora_egress_service(listen_addr, policy, secrets, max_tokens)?;
    let mut server = Server::new(None)
        .map_err(|e| anyhow::anyhow!("Failed to initialize Pingora server: {}", e))?;
    server.bootstrap();
    server.add_service(service);
    server.run_forever();
}

/// Runs the Pingora ingress gateway server synchronously (blocks current thread).
pub fn run_pingora_gateway_server(
    listen_addr: &str,
    routes: HashMap<String, MicroVmServiceBackend>,
) -> Result<()> {
    let service = create_pingora_gateway_service(listen_addr, routes)?;
    let mut server = Server::new(None)
        .map_err(|e| anyhow::anyhow!("Failed to initialize Pingora server: {}", e))?;
    server.bootstrap();
    server.add_service(service);
    server.run_forever();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pingora_egress_proxy_initialization() {
        let policy = EgressPolicy::new(vec!["api.openai.com:443".to_string()]);
        let secrets = vec![("OPENAI_API_KEY".to_string(), "sk-real-secret".to_string())];
        let proxy = PingoraEgressProxy::new(policy, &secrets, Some(1000));

        assert_eq!(proxy.tokens_consumed(), 0);
        assert!(proxy.policy.is_allowed("api.openai.com", 443));
        assert!(!proxy.policy.is_allowed("evil.com", 80));
    }

    #[test]
    fn test_pingora_gateway_routing_match() {
        let mut routes = HashMap::new();
        routes.insert(
            "/api".to_string(),
            MicroVmServiceBackend {
                service_name: "api-service".to_string(),
                target_addr: "127.0.0.1:8080".parse().unwrap(),
                path_prefix: "/api".to_string(),
                strip_prefix: false,
            },
        );
        routes.insert(
            "/web".to_string(),
            MicroVmServiceBackend::new(
                "web-service",
                "127.0.0.1:3000".parse().unwrap(),
                "/web",
            )
            .with_strip_prefix(true),
        );

        let gateway = PingoraMicroVmGateway::new(routes);
        assert_eq!(gateway.routes.len(), 2);
    }

    #[tokio::test]
    async fn test_pingora_egress_body_secret_substitution() {
        let policy = EgressPolicy::new(vec!["api.openai.com:443".to_string()]);
        let secrets = vec![("OPENAI_API_KEY".to_string(), "sk-prod-12345".to_string())];
        let proxy = PingoraEgressProxy::new(policy, &secrets, None);

        let input_body = Bytes::from(r#"{"api_key":"krun-secret:OPENAI_API_KEY","prompt":"hi"}"#);
        let mut body_opt = Some(input_body);

        if let Some(ref chunk) = body_opt {
            let substituted = proxy.secret_substitution.substitute(chunk);
            body_opt = Some(Bytes::from(substituted));
        }

        let result_str = String::from_utf8(body_opt.unwrap().to_vec()).unwrap();
        assert!(result_str.contains("sk-prod-12345"));
        assert!(!result_str.contains("krun-secret:OPENAI_API_KEY"));
    }
}
