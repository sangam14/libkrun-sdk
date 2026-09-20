use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Configuration written to `/.krun_config.json` inside the rootfs.
/// libkrun's built-in init process reads this file as PID 1 to determine what to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrunConfig {
    #[serde(rename = "Cmd")]
    pub cmd: Vec<String>,
    #[serde(rename = "Env", default)]
    pub env: Vec<String>,
    #[serde(rename = "WorkingDir", default = "default_workdir")]
    pub working_dir: String,
}

fn default_workdir() -> String {
    "/".to_string()
}

impl KrunConfig {
    pub const FILE_NAME: &'static str = ".krun_config.json";

    pub fn new(cmd: Vec<String>, env: Vec<String>, working_dir: Option<String>) -> Self {
        Self {
            cmd,
            env,
            working_dir: working_dir.unwrap_or_else(default_workdir),
        }
    }

    /// Writes this config as `/.krun_config.json` inside the given rootfs directory.
    pub fn write_to<P: AsRef<Path>>(&self, rootfs_path: P) -> Result<()> {
        let dest = rootfs_path.as_ref().join(Self::FILE_NAME);
        let data = serde_json::to_string_pretty(self)
            .context("Failed to serialize krun config to JSON")?;
        fs::write(&dest, data)
            .with_context(|| format!("Failed to write krun config to {}", dest.display()))?;
        Ok(())
    }
}

/// Relevant fields parsed from an OCI container image config blob.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OciConfig {
    #[serde(default)]
    pub entrypoint: Vec<String>,
    #[serde(default)]
    pub cmd: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
}

impl OciConfig {
    /// Merges Entrypoint and Cmd following standard OCI container rules:
    /// - If override_cmd is provided, it replaces Entrypoint + Cmd.
    /// - Otherwise, Entrypoint + Cmd are concatenated.
    /// - If both are empty, defaults to `["/bin/sh"]`.
    pub fn resolve_cmd(&self, override_cmd: Option<Vec<String>>) -> Vec<String> {
        if let Some(cmd) = override_cmd {
            if !cmd.is_empty() {
                return cmd;
            }
        }

        let mut final_cmd = self.entrypoint.clone();
        final_cmd.extend(self.cmd.clone());

        if final_cmd.is_empty() {
            vec!["/bin/sh".to_string()]
        } else {
            final_cmd
        }
    }
}

/// Parses a human-readable size string (e.g. "4G", "512M", "1024K", "1048576") into bytes.
pub fn parse_size_to_bytes(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Size string cannot be empty");
    }

    let (num_str, unit) = match s.find(|c: char| c.is_alphabetic()) {
        Some(idx) => (&s[..idx], &s[idx..]),
        None => (s, ""),
    };

    let num: f64 = num_str
        .trim()
        .parse()
        .with_context(|| format!("Invalid numeric size: '{}'", num_str))?;

    let multiplier = match unit.to_uppercase().as_str() {
        "" | "B" => 1u64,
        "K" | "KB" | "KIB" => 1024u64,
        "M" | "MB" | "MIB" => 1024 * 1024u64,
        "G" | "GB" | "GIB" => 1024 * 1024 * 1024u64,
        "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024u64,
        other => anyhow::bail!("Unknown size unit: '{}'", other),
    };

    Ok((num * multiplier as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_krun_config_write() {
        let dir = tempdir().unwrap();
        let cfg = KrunConfig::new(
            vec!["/bin/echo".to_string(), "hello".to_string()],
            vec!["FOO=BAR".to_string()],
            Some("/work".to_string()),
        );
        cfg.write_to(dir.path()).unwrap();

        let written = std::fs::read_to_string(dir.path().join(".krun_config.json")).unwrap();
        assert!(written.contains("\"Cmd\""));
        assert!(written.contains("\"/bin/echo\""));
        assert!(written.contains("\"FOO=BAR\""));
        assert!(written.contains("\"WorkingDir\": \"/work\""));
    }

    #[test]
    fn test_oci_config_resolve_cmd() {
        let oci = OciConfig {
            entrypoint: vec!["/entry.sh".to_string()],
            cmd: vec!["arg1".to_string()],
            ..Default::default()
        };

        // Without override
        let cmd = oci.resolve_cmd(None);
        assert_eq!(cmd, vec!["/entry.sh", "arg1"]);

        // With override
        let override_cmd = vec!["/bin/bash".to_string(), "-c".to_string(), "ls".to_string()];
        let cmd = oci.resolve_cmd(Some(override_cmd.clone()));
        assert_eq!(cmd, override_cmd);
    }

    #[test]
    fn test_parse_size_to_bytes() {
        assert_eq!(parse_size_to_bytes("4G").unwrap(), 4 * 1024 * 1024 * 1024);
        assert_eq!(parse_size_to_bytes("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size_to_bytes("1024K").unwrap(), 1024 * 1024);
        assert_eq!(parse_size_to_bytes("2048").unwrap(), 2048);
        assert_eq!(parse_size_to_bytes("1GiB").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_size_to_bytes("").is_err());
        assert!(parse_size_to_bytes("invalid").is_err());
    }
}
