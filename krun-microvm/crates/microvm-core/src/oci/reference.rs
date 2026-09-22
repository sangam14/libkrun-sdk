use anyhow::{bail, Result};
use std::path::PathBuf;

/// Parsed OCI image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReference {
    pub registry: String,
    pub repository: String,
    pub tag: String,
    pub digest: Option<String>,
    pub is_local_layout: bool,
    pub layout_path: Option<PathBuf>,
}

impl ImageReference {
    pub fn parse(raw: &str) -> Result<Self> {
        if let Some(layout_raw) = raw.strip_prefix("oci:") {
            let (path_part, tag) = if let Some(colon_idx) = layout_raw.rfind(':') {
                let after = &layout_raw[colon_idx + 1..];
                if !after.contains('/') && !after.contains('\\') {
                    (&layout_raw[..colon_idx], after.to_string())
                } else {
                    (layout_raw, "latest".to_string())
                }
            } else {
                (layout_raw, "latest".to_string())
            };
            let path = PathBuf::from(path_part);
            return Ok(Self {
                registry: "local".to_string(),
                repository: path.to_string_lossy().to_string(),
                tag,
                digest: None,
                is_local_layout: true,
                layout_path: Some(path),
            });
        }

        let (registry, rest) = if let Some(slash_idx) = raw.find('/') {
            let potential_reg = &raw[..slash_idx];
            if potential_reg.contains('.')
                || potential_reg.contains(':')
                || potential_reg == "localhost"
            {
                (potential_reg.to_string(), &raw[slash_idx + 1..])
            } else {
                ("registry-1.docker.io".to_string(), raw)
            }
        } else {
            ("registry-1.docker.io".to_string(), raw)
        };

        let (repo_tag_part, digest) = if let Some(at_idx) = rest.find('@') {
            let digest_part = &rest[at_idx + 1..];
            if !digest_part.starts_with("sha256:") {
                bail!(
                    "Unsupported digest algorithm in '{}': expected 'sha256:'",
                    raw
                );
            }
            (&rest[..at_idx], Some(digest_part.to_string()))
        } else {
            (rest, None)
        };

        let (repo_part, tag) = if let Some(colon_idx) = repo_tag_part.rfind(':') {
            (
                &repo_tag_part[..colon_idx],
                repo_tag_part[colon_idx + 1..].to_string(),
            )
        } else {
            (repo_tag_part, "latest".to_string())
        };

        if repo_part.is_empty() {
            bail!("Empty image repository in '{}'", raw);
        }

        // Docker Hub official images need "library/" prefix
        let repository = if registry == "registry-1.docker.io" && !repo_part.contains('/') {
            format!("library/{}", repo_part)
        } else {
            repo_part.to_string()
        };

        Ok(Self {
            registry,
            repository,
            tag,
            digest,
            is_local_layout: false,
            layout_path: None,
        })
    }

    pub fn canonical_name(&self) -> String {
        if self.is_local_layout {
            format!("oci:{}:{}", self.repository, self.tag)
        } else if let Some(ref d) = self.digest {
            if self.tag != "latest" {
                format!("{}/{}:{}@{}", self.registry, self.repository, self.tag, d)
            } else {
                format!("{}/{}@{}", self.registry, self.repository, d)
            }
        } else {
            format!("{}/{}:{}", self.registry, self.repository, self.tag)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_references() {
        let r1 = ImageReference::parse("alpine").unwrap();
        assert_eq!(r1.registry, "registry-1.docker.io");
        assert_eq!(r1.repository, "library/alpine");
        assert_eq!(r1.tag, "latest");
        assert_eq!(r1.digest, None);

        let r2 = ImageReference::parse("alpine:3.19").unwrap();
        assert_eq!(r2.repository, "library/alpine");
        assert_eq!(r2.tag, "3.19");
        assert_eq!(r2.digest, None);

        let r3 = ImageReference::parse("ghcr.io/krun/demo:v1").unwrap();
        assert_eq!(r3.registry, "ghcr.io");
        assert_eq!(r3.repository, "krun/demo");
        assert_eq!(r3.tag, "v1");
        assert_eq!(r3.digest, None);

        let r4 = ImageReference::parse("oci:/tmp/my-image:v2").unwrap();
        assert!(r4.is_local_layout);
        assert_eq!(
            r4.layout_path.as_ref().unwrap(),
            &PathBuf::from("/tmp/my-image")
        );
        assert_eq!(r4.tag, "v2");
        assert_eq!(r4.canonical_name(), "oci:/tmp/my-image:v2");

        // Digest pins
        let r5 = ImageReference::parse(
            "alpine@sha256:7144f7e135b309e81b9b7193eeeabebdd40f3b94c4d6ce14edd328b0547aa292",
        )
        .unwrap();
        assert_eq!(r5.repository, "library/alpine");
        assert_eq!(r5.tag, "latest");
        assert_eq!(
            r5.digest,
            Some("sha256:7144f7e135b309e81b9b7193eeeabebdd40f3b94c4d6ce14edd328b0547aa292".into())
        );
        assert_eq!(r5.canonical_name(), "registry-1.docker.io/library/alpine@sha256:7144f7e135b309e81b9b7193eeeabebdd40f3b94c4d6ce14edd328b0547aa292");

        let r6 = ImageReference::parse("quay.io/coreos/etcd:v3.5.0@sha256:abcdef123456").unwrap();
        assert_eq!(r6.registry, "quay.io");
        assert_eq!(r6.repository, "coreos/etcd");
        assert_eq!(r6.tag, "v3.5.0");
        assert_eq!(r6.digest, Some("sha256:abcdef123456".into()));
        assert_eq!(
            r6.canonical_name(),
            "quay.io/coreos/etcd:v3.5.0@sha256:abcdef123456"
        );
    }
}
