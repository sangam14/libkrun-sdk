use super::client::OciClient;
use super::reference::ImageReference;
use crate::rootfs::extract::extract_layer;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Metadata stored alongside a cached OCI artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub reference: String,
    pub digest: String,
    pub files: Vec<String>,
    pub created_at: u64,
}

pub struct OciArtifact;

impl OciArtifact {
    /// Lists all cached OCI artifacts in `cache_base/artifacts`.
    pub fn list_cached(cache_base: &Path) -> Result<Vec<ArtifactMetadata>> {
        let art_dir = cache_base.join("artifacts");
        if !art_dir.exists() {
            return Ok(Vec::new());
        }

        let mut list = Vec::new();
        for entry in fs::read_dir(&art_dir)? {
            let entry = entry?;
            let meta_file = entry.path().join(".krun_artifact.json");
            if meta_file.exists() {
                if let Ok(content) = fs::read_to_string(&meta_file) {
                    if let Ok(meta) = serde_json::from_str::<ArtifactMetadata>(&content) {
                        list.push(meta);
                    }
                }
            }
        }
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(list)
    }
}

pub(crate) async fn pull_artifact_impl(
    client: &OciClient,
    reference: &ImageReference,
    cache_base: &Path,
) -> Result<PathBuf> {
    let token = client.get_token(reference).await?;
    let token_ref = token.as_deref();

    // 1. Fetch manifest
    let (manifest, manifest_digest) = client.fetch_manifest(reference, token_ref).await?;
    let safe_digest = manifest_digest.replace(':', "_");
    let artifact_dir = cache_base.join("artifacts").join(&safe_digest);

    // Cache hit check
    let meta_path = artifact_dir.join(".krun_artifact.json");
    if artifact_dir.exists() && meta_path.exists() {
        tracing::info!("Artifact cache hit for {}", manifest_digest);
        return Ok(artifact_dir);
    }

    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("Failed to create artifact dir: {}", artifact_dir.display()))?;

    let mut saved_files = Vec::new();
    tracing::info!(
        "Downloading OCI artifact {} ({} blobs)...",
        reference.canonical_name(),
        manifest.layers.len()
    );

    for (i, layer) in manifest.layers.iter().enumerate() {
        // Resolve filename from annotations or default
        let filename = layer
            .annotations
            .as_ref()
            .and_then(|a| {
                a.get("org.opencontainers.image.title")
                    .or_else(|| a.get("io.deis.oras.content.unpack"))
            })
            .cloned()
            .unwrap_or_else(|| {
                let is_tar = layer
                    .media_type
                    .as_ref()
                    .map_or(false, |m| m.contains("tar") || m.contains("layer"));
                if is_tar {
                    format!("layer_{}.tar", i)
                } else {
                    let hash_part = layer.digest.strip_prefix("sha256:").unwrap_or(&layer.digest);
                    let short_hash = &hash_part[..hash_part.len().min(12)];
                    format!("blob_{}.bin", short_hash)
                }
            });

        tracing::info!(
            "Fetching artifact blob [{}/{}] -> {}",
            i + 1,
            manifest.layers.len(),
            filename
        );
        let blob_bytes = client
            .fetch_blob_bytes(reference, &layer.digest, token_ref)
            .await?;

        let is_tar = layer
            .media_type
            .as_ref()
            .map_or(false, |m| m.contains("tar"))
            || filename.ends_with(".tar")
            || filename.ends_with(".tar.gz");

        if is_tar && (filename.starts_with("layer_") || filename.ends_with(".tar")) {
            // Unpack tar layer into the artifact directory
            let cursor = std::io::Cursor::new(blob_bytes);
            let is_gz = layer
                .media_type
                .as_ref()
                .map_or(false, |m| m.contains("gzip"))
                || filename.ends_with(".tar.gz");
            extract_layer(cursor, &artifact_dir, is_gz)?;
            saved_files.push(format!("{}/ (unpacked)", filename));
        } else {
            // Save raw file
            let target_file = artifact_dir.join(&filename);
            if let Some(parent) = target_file.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target_file, &blob_bytes)?;
            saved_files.push(filename);
        }
    }

    // Save metadata
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let meta = ArtifactMetadata {
        reference: reference.canonical_name(),
        digest: manifest_digest,
        files: saved_files,
        created_at: now,
    };
    let _ = fs::write(&meta_path, serde_json::to_string_pretty(&meta)?);

    Ok(artifact_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_artifact_list_cached() {
        let dir = tempdir().unwrap();
        let cache_base = dir.path();

        let art1 = cache_base.join("artifacts/sha256_aaaa");
        fs::create_dir_all(&art1).unwrap();
        let meta1 = ArtifactMetadata {
            reference: "ghcr.io/org/model:v1".to_string(),
            digest: "sha256:aaaa".to_string(),
            files: vec!["model.gguf".to_string()],
            created_at: 100,
        };
        fs::write(
            art1.join(".krun_artifact.json"),
            serde_json::to_string(&meta1).unwrap(),
        )
        .unwrap();

        let art2 = cache_base.join("artifacts/sha256_bbbb");
        fs::create_dir_all(&art2).unwrap();
        let meta2 = ArtifactMetadata {
            reference: "ghcr.io/org/dataset:v2".to_string(),
            digest: "sha256:bbbb".to_string(),
            files: vec!["data.parquet".to_string()],
            created_at: 200,
        };
        fs::write(
            art2.join(".krun_artifact.json"),
            serde_json::to_string(&meta2).unwrap(),
        )
        .unwrap();

        let list = OciArtifact::list_cached(cache_base).unwrap();
        assert_eq!(list.len(), 2);
        // Should be sorted by created_at desc
        assert_eq!(list[0].reference, "ghcr.io/org/dataset:v2");
        assert_eq!(list[1].reference, "ghcr.io/org/model:v1");
        assert_eq!(list[0].files, vec!["data.parquet"]);
    }
}
