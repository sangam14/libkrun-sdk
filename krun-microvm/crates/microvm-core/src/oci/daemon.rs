use super::client::parse_oci_config_from_json;
use crate::config::OciConfig;
use crate::rootfs::extract::extract_layer;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::Archive;

#[derive(Debug, Deserialize)]
struct DockerSaveManifestEntry {
    #[serde(rename = "Config")]
    config: String,
    #[serde(rename = "Layers")]
    layers: Vec<String>,
}

pub struct DockerDaemon;

impl DockerDaemon {
    /// Attempts to fetch a locally built or cached image from the Docker or Podman daemon.
    pub fn try_fetch_and_unpack(
        image_name: &str,
        cache_base: &Path,
    ) -> Result<Option<(PathBuf, OciConfig)>> {
        // Try `docker save <image_name>`
        let mut child = match Command::new("docker")
            .args(["save", image_name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                // Try podman
                match Command::new("podman")
                    .args(["save", image_name])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => return Ok(None),
                }
            }
        };

        let stdout = match child.stdout.take() {
            Some(out) => out,
            None => return Ok(None),
        };

        // Extract the docker save archive into a staging bundle
        let temp_bundle = tempfile::tempdir()?;
        let mut archive = Archive::new(stdout);

        if let Err(e) = archive.unpack(temp_bundle.path()) {
            tracing::debug!("Failed to unpack local docker archive for {}: {}", image_name, e);
            let _ = child.wait();
            return Ok(None);
        }

        let status = child.wait()?;
        if !status.success() {
            return Ok(None);
        }

        let manifest_path = temp_bundle.path().join("manifest.json");
        if !manifest_path.exists() {
            return Ok(None);
        }

        let manifest_data = fs::read_to_string(&manifest_path)?;
        let entries: Vec<DockerSaveManifestEntry> = serde_json::from_str(&manifest_data)
            .context("Failed to parse manifest.json from docker save")?;

        if entries.is_empty() {
            return Ok(None);
        }

        let entry = &entries[0];

        // Read and parse config JSON
        let config_path = temp_bundle.path().join(&entry.config);
        let config_data = fs::read_to_string(&config_path)
            .with_context(|| format!("Missing config file: {}", config_path.display()))?;
        let full_config: serde_json::Value = serde_json::from_str(&config_data)?;
        let oci_config = parse_oci_config_from_json(&full_config);

        // Derive content-addressable hash from config filename
        let config_stem = Path::new(&entry.config)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("local_image")
            .replace(':', "_");

        let safe_name = format!("local_{}", config_stem);
        let rootfs_dir = cache_base.join("rootfs").join(&safe_name);
        let config_file = cache_base.join("configs").join(format!("{}.json", safe_name));

        if rootfs_dir.exists() && config_file.exists() {
            return Ok(Some((rootfs_dir, oci_config)));
        }

        // Extract layers into staging
        let staging_rootfs = cache_base.join("staging").join(&safe_name);
        if staging_rootfs.exists() {
            let _ = fs::remove_dir_all(&staging_rootfs);
        }
        fs::create_dir_all(&staging_rootfs)?;

        tracing::info!("Extracting {} layers from local Docker daemon...", entry.layers.len());
        for layer_rel in &entry.layers {
            let layer_path = temp_bundle.path().join(layer_rel);
            let file = File::open(&layer_path)?;
            let reader = BufReader::new(file);

            // Layers in docker save are either uncompressed tar or tar.gz
            let is_gzipped = layer_rel.ends_with(".gz");
            extract_layer(reader, &staging_rootfs, is_gzipped)?;
        }

        // Commit rootfs atomically
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

        Ok(Some((rootfs_dir, oci_config)))
    }
}
