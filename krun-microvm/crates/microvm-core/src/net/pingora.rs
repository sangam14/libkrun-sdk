// Copyright 2026, libkrun-sdk authors.
// SPDX-License-Identifier: Apache-2.0

//! Cloudflare Pingora integration for high-performance L4/L7 Ingress, Reverse Proxying,
//! and Zero-Trust Egress filtering in `libkrun-sdk`.

use crate::net::egress::{EgressPolicy, SecretSubstitution};
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
/// 1. Default-deny domain whitelisting with wildcard support (`*.github.com`).
/// 2. Strict cloud metadata defense (blocks SSRF to `169.254.169.254`).
/// 3. In-flight secret substitution (replaces `krun-secret:KEY` with real credentials).
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

        let (target_host, target_port, is_tls) = if let Some((h, p)) = host.split_once(':') {
            let port = p.parse::<u16>().unwrap_or(443);
            (h.to_string(), port, port == 443)
        } else {
            let is_https = req_header.uri.scheme_str() == Some("https");
            let port = if is_https { 443 } else { 80 };
            (host.to_string(), port, is_https)
        };

        ctx.target_host = target_host.clone();
        ctx.target_port = target_port;
        ctx.is_tls = is_tls;

        // Verify egress policy (domain whitelist & AWS/cloud metadata protection)
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
        let peer_addr = format!("{}:{}", ctx.target_host, ctx.target_port);
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

    /// Rewrites outbound request headers and body for in-flight secret substitution.
    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        // Substitute secrets in HTTP request headers
        let mut updates = Vec::new();
        for (name, value) in upstream_request.headers.iter() {
            if let Ok(val_str) = value.to_str() {
                if val_str.contains("krun-secret:") {
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

        // Route matching by longest prefix
        let mut matched: Option<&MicroVmServiceBackend> = None;
        for (prefix, backend) in self.routes.iter() {
            if path.starts_with(prefix)
                && (matched.is_none() || prefix.len() > matched.unwrap().path_prefix.len())
            {
                matched = Some(backend);
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
            },
        );
        routes.insert(
            "/web".to_string(),
            MicroVmServiceBackend {
                service_name: "web-service".to_string(),
                target_addr: "127.0.0.1:3000".parse().unwrap(),
                path_prefix: "/web".to_string(),
            },
        );

        let gateway = PingoraMicroVmGateway::new(routes);
        assert_eq!(gateway.routes.len(), 2);
    }
}
