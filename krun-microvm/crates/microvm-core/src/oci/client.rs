use super::reference::ImageReference;
use crate::config::OciConfig;
use crate::rootfs::extract::extract_layer;
use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, WWW_AUTHENTICATE};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OciClient {
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct DockerTokenResponse {
    token: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestIndex {
    manifests: Option<Vec<ManifestIndexEntry>>,
}

#[derive(Debug, Deserialize)]
struct ManifestIndexEntry {
    digest: String,
    platform: Option<Platform>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Platform {
    architecture: String,
    os: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SingleManifest {
    pub(crate) config: BlobDescriptor,
    pub(crate) layers: Vec<BlobDescriptor>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct BlobDescriptor {
    #[serde(rename = "mediaType")]
    pub(crate) media_type: Option<String>,
    pub(crate) digest: String,
    pub(crate) size: u64,
    pub(crate) annotations: Option<std::collections::HashMap<String, String>>,
}

impl Default for OciClient {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct FileLock {
    file: std::fs::File,
}

impl FileLock {
    pub(crate) fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        unsafe {
            use std::os::unix::io::AsRawFd;
            if libc::flock(file.as_raw_fd(), libc::LOCK_EX) != 0 {
                bail!(
                    "Failed to acquire advisory lock on {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                );
            }
        }
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            use std::os::unix::io::AsRawFd;
            let _ = libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

static STAGING_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl OciClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("krun-microvm/0.1.0")
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    /// Pulls an arbitrary OCI artifact (e.g. model weights, dataset, toolchain) into cache_base/artifacts/<digest>.
    pub async fn pull_artifact(
        &self,
        reference: &ImageReference,
        cache_base: &Path,
    ) -> Result<PathBuf> {
        super::artifact::pull_artifact_impl(self, reference, cache_base).await
    }

    /// Obtains an anonymous bearer token for pulling from any OCI compliant registry.
    pub(crate) async fn get_token(&self, reference: &ImageReference) -> Result<Option<String>> {
        // 1. Probe the registry /v2/ endpoint to receive standard Www-Authenticate challenge
        let ping_url = format!("https://{}/v2/", reference.registry);
        if let Ok(resp) = self.http.get(&ping_url).send().await {
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                if let Some(auth_hdr) = resp.headers().get(WWW_AUTHENTICATE) {
                    if let Ok(hdr_str) = auth_hdr.to_str() {
                        if let Ok(Some(tok)) =
                            self.token_from_www_authenticate(hdr_str, reference).await
                        {
                            return Ok(Some(tok));
                        }
                    }
                }
            }
        }

        // 2. Direct Docker Hub fallback if probe didn't return challenge
        if reference.registry == "registry-1.docker.io" {
            let url = format!(
                "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{}:pull",
                reference.repository
            );
            if let Ok(resp) = self.http.get(&url).send().await {
                if resp.status().is_success() {
                    let body: DockerTokenResponse = resp.json().await?;
                    return Ok(body.token.or(body.access_token));
                }
            }
        }

        Ok(None)
    }

    /// Parses RFC 7235 Www-Authenticate header and fetches a bearer token from the indicated realm.
    async fn token_from_www_authenticate(
        &self,
        auth_hdr: &str,
        reference: &ImageReference,
    ) -> Result<Option<String>> {
        let auth_hdr = auth_hdr.trim();
        if !auth_hdr.starts_with("Bearer ") && !auth_hdr.starts_with("bearer ") {
            return Ok(None);
        }
        let params_part = &auth_hdr[7..];
        let mut realm = None;
        let mut service = None;
        let mut scope = None;

        for part in params_part.split(',') {
            if let Some((k, v)) = part.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"');
                match key {
                    "realm" => realm = Some(val.to_string()),
                    "service" => service = Some(val.to_string()),
                    "scope" => scope = Some(val.to_string()),
                    _ => {}
                }
            }
        }

        let realm_url = match realm {
            Some(r) => r,
            None => return Ok(None),
        };

        let mut req = self.http.get(&realm_url);
        if let Some(s) = service {
            req = req.query(&[("service", s)]);
        }
        let final_scope =
            scope.unwrap_or_else(|| format!("repository:{}:pull", reference.repository));
        req = req.query(&[("scope", final_scope)]);

        let resp = req.send().await?;
        if resp.status().is_success() {
            let body: DockerTokenResponse = resp.json().await?;
            return Ok(body.token.or(body.access_token));
        }

        Ok(None)
    }

    fn auth_headers(&self, token: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(t) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", t)).unwrap(),
            );
        }
        headers
    }

    /// Pulls the image and returns the path to the cached, extracted rootfs along with its OCI configuration.
    pub async fn pull_and_unpack(
        &self,
        reference: &ImageReference,
        cache_base: &Path,
    ) -> Result<(PathBuf, OciConfig)> {
        // 0. Try local Docker / Podman daemon first
        let stripped_repo = reference
            .repository
            .strip_prefix("library/")
            .unwrap_or(&reference.repository);
        let local_names = [
            format!("{}:{}", stripped_repo, reference.tag),
            format!("{}:{}", reference.repository, reference.tag),
            stripped_repo.to_string(),
            reference.repository.clone(),
            reference.canonical_name(),
        ];
        for name in &local_names {
            if let Ok(Some((rootfs, cfg))) =
                super::daemon::DockerDaemon::try_fetch_and_unpack(name, cache_base)
            {
                tracing::info!("Using local image '{}' from Docker daemon", name);
                return Ok((rootfs, cfg));
            }
        }

        let token = self.get_token(reference).await?;
        let token_ref = token.as_deref();

        // 1. Fetch Manifest
        let (manifest, manifest_digest) = self.fetch_manifest(reference, token_ref).await?;

        let safe_digest = manifest_digest.replace(':', "_");
        let rootfs_dir = cache_base.join("rootfs").join(&safe_digest);
        let config_file = cache_base
            .join("configs")
            .join(format!("{}.json", safe_digest));

        // Acquire advisory lock to deduplicate concurrent pulls of this image
        let lock_path = cache_base
            .join("locks")
            .join(format!("{}.lock", safe_digest));
        let _lock = FileLock::acquire(&lock_path)?;

        // Cache hit check after acquiring lock
        if rootfs_dir.exists() && config_file.exists() {
            let config_data = fs::read_to_string(&config_file)?;
            if let Ok(cfg) = serde_json::from_str::<OciConfig>(&config_data) {
                tracing::info!("RootFS cache hit for manifest {}", manifest_digest);
                return Ok((rootfs_dir, cfg));
            }
        }

        // 2. Fetch OCI Config blob (verifies digest)
        let config_bytes = self
            .fetch_blob(reference, &manifest.config.digest, token_ref)
            .await?;
        let full_config: serde_json::Value =
            serde_json::from_slice(&config_bytes).context("Failed to parse image config JSON")?;

        let oci_config = parse_oci_config_from_json(&full_config);

        // 3. Unpack layers into isolated staging rootfs
        let counter = STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pid = std::process::id();
        let staging_rootfs = cache_base
            .join("staging")
            .join(format!("{safe_digest}_{pid}_{counter}"));
        if staging_rootfs.exists() {
            let _ = fs::remove_dir_all(&staging_rootfs);
        }
        fs::create_dir_all(&staging_rootfs)?;

        tracing::info!(
            "Extracting {} layers for {} (streaming)...",
            manifest.layers.len(),
            reference.canonical_name()
        );
        for (i, layer) in manifest.layers.iter().enumerate() {
            tracing::info!(
                "Extracting layer [{}/{}] ({})",
                i + 1,
                manifest.layers.len(),
                layer.digest
            );

            let layer_hash = layer
                .digest
                .strip_prefix("sha256:")
                .unwrap_or(&layer.digest);
            let blob_cache_file = cache_base.join("blobs").join("sha256").join(layer_hash);

            let blob_bytes = if blob_cache_file.exists() {
                tracing::debug!("Layer blob cache hit: {}", layer.digest);
                bytes::Bytes::from(std::fs::read(&blob_cache_file)?)
            } else {
                let downloaded = self
                    .fetch_blob_bytes(reference, &layer.digest, token_ref)
                    .await?;
                if let Some(parent) = blob_cache_file.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let temp_blob = cache_base
                    .join("staging")
                    .join(format!("blob_{layer_hash}_{pid}_{counter}.tmp"));
                if let Some(parent) = temp_blob.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&temp_blob, &downloaded).is_ok() {
                    let _ = std::fs::rename(&temp_blob, &blob_cache_file);
                }
                downloaded
            };

            let cursor = std::io::Cursor::new(blob_bytes);
            extract_layer(cursor, &staging_rootfs, true)?;
        }

        // 4. Atomically commit rootfs
        if rootfs_dir.exists() {
            let _ = fs::remove_dir_all(&rootfs_dir);
        }
        if let Some(parent) = rootfs_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&staging_rootfs, &rootfs_dir)?;

        // Save config
        if let Some(parent) = config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&config_file, serde_json::to_string_pretty(&oci_config)?)?;

        Ok((rootfs_dir, oci_config))
    }

    pub(crate) async fn fetch_manifest(
        &self,
        reference: &ImageReference,
        token: Option<&str>,
    ) -> Result<(SingleManifest, String)> {
        let target_tag_or_digest = if let Some(ref d) = reference.digest {
            d.as_str()
        } else {
            reference.tag.as_str()
        };

        let manifest_url = format!(
            "https://{}/v2/{}/manifests/{}",
            reference.registry, reference.repository, target_tag_or_digest
        );

        let mut headers = self.auth_headers(token);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "application/vnd.docker.distribution.manifest.v2+json, \
                 application/vnd.docker.distribution.manifest.list.v2+json, \
                 application/vnd.oci.image.manifest.v1+json, \
                 application/vnd.oci.image.index.v1+json",
            ),
        );

        let resp = self
            .http
            .get(&manifest_url)
            .headers(headers.clone())
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("Failed to fetch manifest: HTTP {}", resp.status());
        }

        let body_bytes = resp.bytes().await?;
        let digest_str = format!("sha256:{:x}", Sha256::digest(&body_bytes));

        // If pinned by digest, verify digest matches
        if let Some(ref expected_digest) = reference.digest {
            if &digest_str != expected_digest && target_tag_or_digest.starts_with("sha256:") {
                bail!(
                    "Cryptographic digest mismatch for manifest: expected {}, got {}",
                    expected_digest,
                    digest_str
                );
            }
        }

        // Check if this is an image index / multi-arch manifest list
        if let Ok(index) = serde_json::from_slice::<ManifestIndex>(&body_bytes) {
            if let Some(entries) = index.manifests {
                let target_arch = match std::env::consts::ARCH {
                    "aarch64" => "arm64",
                    "x86_64" => "amd64",
                    other => other,
                };

                let matched_entry = entries.iter().find(|e| {
                    if let Some(ref p) = e.platform {
                        p.os == "linux" && p.architecture == target_arch
                    } else {
                        false
                    }
                });

                let chosen_digest = match matched_entry {
                    Some(entry) => &entry.digest,
                    None => {
                        // fallback to first linux manifest
                        entries
                            .iter()
                            .find(|e| e.platform.as_ref().is_some_and(|p| p.os == "linux"))
                            .map(|e| &e.digest)
                            .unwrap_or(&entries[0].digest)
                    }
                };

                // Fetch the concrete single manifest for this platform
                let child_url = format!(
                    "https://{}/v2/{}/manifests/{}",
                    reference.registry, reference.repository, chosen_digest
                );
                let child_resp = self.http.get(&child_url).headers(headers).send().await?;
                let child_bytes = child_resp.bytes().await?;
                let single: SingleManifest = serde_json::from_slice(&child_bytes)
                    .context("Failed to parse concrete single manifest")?;
                return Ok((single, chosen_digest.clone()));
            }
        }

        let single: SingleManifest =
            serde_json::from_slice(&body_bytes).context("Failed to parse single manifest")?;
        Ok((single, digest_str))
    }

    async fn fetch_blob(
        &self,
        reference: &ImageReference,
        digest: &str,
        token: Option<&str>,
    ) -> Result<Vec<u8>> {
        let bytes = self.fetch_blob_bytes(reference, digest, token).await?;
        Ok(bytes.to_vec())
    }

    pub(crate) async fn fetch_blob_bytes(
        &self,
        reference: &ImageReference,
        digest: &str,
        token: Option<&str>,
    ) -> Result<bytes::Bytes> {
        let url = format!(
            "https://{}/v2/{}/blobs/{}",
            reference.registry, reference.repository, digest
        );

        let headers = self.auth_headers(token);
        let resp = self.http.get(&url).headers(headers).send().await?;
        if !resp.status().is_success() {
            bail!("Failed to download blob {}: HTTP {}", digest, resp.status());
        }

        let bytes = resp.bytes().await?;

        // Cryptographically verify SHA-256 digest of downloaded payload
        if let Some(expected_hex) = digest.strip_prefix("sha256:") {
            let actual_hex = format!("{:x}", Sha256::digest(&bytes));
            if actual_hex != expected_hex {
                bail!(
                    "Cryptographic digest mismatch for blob: expected sha256:{}, got sha256:{}",
                    expected_hex,
                    actual_hex
                );
            }
        }

        Ok(bytes)
    }
}

pub(crate) fn parse_oci_config_from_json(json: &serde_json::Value) -> OciConfig {
    let config_node = json.get("config").unwrap_or(json);

    let entrypoint = config_node
        .get("Entrypoint")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let cmd = config_node
        .get("Cmd")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let env = config_node
        .get("Env")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let working_dir = config_node
        .get("WorkingDir")
        .and_then(|v| v.as_str())
        .map(String::from);

    let user = config_node
        .get("User")
        .and_then(|v| v.as_str())
        .map(String::from);

    OciConfig {
        entrypoint,
        cmd,
        env,
        working_dir,
        user,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_www_authenticate_header() {
        let hdr = r#"Bearer realm="https://quay.io/v2/auth",service="quay.io""#;
        let mut realm = None;
        let mut service = None;
        for part in hdr["Bearer ".len()..].split(',') {
            if let Some((k, v)) = part.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"');
                if key == "realm" {
                    realm = Some(val.to_string());
                }
                if key == "service" {
                    service = Some(val.to_string());
                }
            }
        }
        assert_eq!(realm.as_deref(), Some("https://quay.io/v2/auth"));
        assert_eq!(service.as_deref(), Some("quay.io"));
    }

    #[test]
    fn test_file_lock_lifecycle() {
        let temp_dir = tempfile::tempdir().unwrap();
        let lock_file = temp_dir.path().join("test.lock");

        // 1. Acquire lock
        let lock1 = FileLock::acquire(&lock_file).expect("Failed to acquire first lock");

        // 2. Attempt non-blocking lock from another FD -> should fail
        let file2 = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_file)
            .unwrap();
        unsafe {
            use std::os::unix::io::AsRawFd;
            let ret = libc::flock(file2.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB);
            assert_ne!(ret, 0, "Second non-blocking lock should fail while held");
        }

        // 3. Drop lock1
        drop(lock1);

        // 4. Now acquiring lock again should succeed
        let _lock3 =
            FileLock::acquire(&lock_file).expect("Should acquire lock after previous was dropped");
    }

    #[test]
    fn test_parse_oci_config() {
        let json = serde_json::json!({
            "config": {
                "Entrypoint": ["/bin/sh", "-c"],
                "Cmd": ["echo hello"],
                "Env": ["PATH=/usr/bin:/bin", "FOO=BAR"],
                "WorkingDir": "/workspace",
                "User": "1000:1000"
            }
        });
        let parsed = parse_oci_config_from_json(&json);
        assert_eq!(parsed.entrypoint, vec!["/bin/sh", "-c"]);
        assert_eq!(parsed.cmd, vec!["echo hello"]);
        assert_eq!(parsed.env, vec!["PATH=/usr/bin:/bin", "FOO=BAR"]);
        assert_eq!(parsed.working_dir, Some("/workspace".to_string()));
        assert_eq!(parsed.user, Some("1000:1000".to_string()));
    }
}
