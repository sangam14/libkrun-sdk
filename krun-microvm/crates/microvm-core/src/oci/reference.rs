use anyhow::{bail, Result};
use std::path::PathBuf;

/// Parsed OCI image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReference {
    pub registry: String,
    pub repository: String,
    pub tag: String,
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
                is_local_layout: true,
                layout_path: Some(path),
            });
        }

        let (registry, rest) = if let Some(slash_idx) = raw.find('/') {
            let potential_reg = &raw[..slash_idx];
            if potential_reg.contains('.') || potential_reg.contains(':') || potential_reg == "localhost" {
                (potential_reg.to_string(), &raw[slash_idx + 1..])
            } else {
                ("registry-1.docker.io".to_string(), raw)
            }
        } else {
            ("registry-1.docker.io".to_string(), raw)
        };

        let (repo_part, tag) = if let Some(colon_idx) = rest.rfind(':') {
            (&rest[..colon_idx], rest[colon_idx + 1..].to_string())
        } else {
            (rest, "latest".to_string())
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
            is_local_layout: false,
            layout_path: None,
        })
    }

    pub fn canonical_name(&self) -> String {
        if self.is_local_layout {
            format!("oci:{}:{}", self.repository, self.tag)
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

        let r2 = ImageReference::parse("alpine:3.19").unwrap();
        assert_eq!(r2.repository, "library/alpine");
        assert_eq!(r2.tag, "3.19");

        let r3 = ImageReference::parse("ghcr.io/krun/demo:v1").unwrap();
        assert_eq!(r3.registry, "ghcr.io");
        assert_eq!(r3.repository, "krun/demo");
        assert_eq!(r3.tag, "v1");

        let r4 = ImageReference::parse("oci:/tmp/my-image:v2").unwrap();
        assert!(r4.is_local_layout);
        assert_eq!(r4.layout_path.as_ref().unwrap(), &PathBuf::from("/tmp/my-image"));
        assert_eq!(r4.tag, "v2");
        assert_eq!(r4.canonical_name(), "oci:/tmp/my-image:v2");
    }
}
