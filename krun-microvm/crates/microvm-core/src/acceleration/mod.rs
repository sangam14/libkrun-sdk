use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Supported accelerated container filesystem formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccelerationFormat {
    /// Dragonfly Nydus RAFS v6 format (fully compatible with Linux in-kernel EROFS).
    #[default]
    RafsV6,
    /// Native in-kernel Enhanced Read-Only File System (EROFS).
    Erofs,
    /// Seekable zlib/gzip indexed layers (zran/stargz compatibility mode).
    Zran,
}

impl AccelerationFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RafsV6 => "rafsv6",
            Self::Erofs => "erofs",
            Self::Zran => "zran",
        }
    }
}

impl std::str::FromStr for AccelerationFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rafsv6" | "rafs" | "nydus" => Ok(Self::RafsV6),
            "erofs" => Ok(Self::Erofs),
            "zran" | "stargz" => Ok(Self::Zran),
            other => bail!(
                "Unknown acceleration format: '{}'. Supported: rafsv6, erofs, zran",
                other
            ),
        }
    }
}

/// Dragonfly Nydus / EROFS image acceleration configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAcceleration {
    /// Format of the accelerated container image (default: RafsV6).
    #[serde(default)]
    pub format: AccelerationFormat,

    /// Whether lazy loading (on-demand chunk streaming) is enabled.
    #[serde(default = "default_lazy_load")]
    pub lazy_load: bool,

    /// Optional local path to the RAFS v6 / EROFS metadata bootstrap image.
    #[serde(default)]
    pub bootstrap_path: Option<PathBuf>,

    /// Host directory where downloaded chunk blobs are cached and deduplicated.
    #[serde(default)]
    pub chunk_cache_dir: Option<PathBuf>,

    /// Default chunk or macro-chunk size in bytes (e.g. 4MB for containers, 64MB for LLM weights).
    #[serde(default)]
    pub chunk_size_bytes: Option<u64>,

    /// List of prioritized file/directory paths to prefetch ahead of container execution.
    #[serde(default)]
    pub prefetch_list: Vec<String>,
}

fn default_lazy_load() -> bool {
    true
}

impl Default for ImageAcceleration {
    fn default() -> Self {
        Self {
            format: AccelerationFormat::default(),
            lazy_load: true,
            bootstrap_path: None,
            chunk_cache_dir: None,
            chunk_size_bytes: Some(4 * 1024 * 1024), // 4 MiB default chunk size
            prefetch_list: Vec::new(),
        }
    }
}

impl ImageAcceleration {
    /// Magic bytes for Dragonfly Nydus RAFS superblock (`0x52414653` / "RAFS").
    pub const RAFS_SUPER_MAGIC: u32 = 0x5241_4653;
    /// Magic bytes for Linux EROFS filesystem superblock (`0xE0F5E1E2`).
    pub const EROFS_SUPER_MAGIC_V1: u32 = 0xE0F5_E1E2;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_format(mut self, format: AccelerationFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_lazy_load(mut self, lazy: bool) -> Self {
        self.lazy_load = lazy;
        self
    }

    pub fn with_bootstrap(mut self, path: impl Into<PathBuf>) -> Self {
        self.bootstrap_path = Some(path.into());
        self
    }

    pub fn with_chunk_cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.chunk_cache_dir = Some(path.into());
        self
    }

    pub fn with_chunk_size(mut self, bytes: u64) -> Self {
        self.chunk_size_bytes = Some(bytes);
        self
    }

    pub fn with_prefetch(mut self, paths: Vec<String>) -> Self {
        self.prefetch_list = paths;
        self
    }

    /// Verifies if a bootstrap metadata file starts with a valid RAFS or EROFS superblock magic header.
    pub fn inspect_bootstrap_magic(path: impl AsRef<Path>) -> Result<AccelerationFormat> {
        let p = path.as_ref();
        let mut file = File::open(p)
            .with_context(|| format!("Failed to open bootstrap file: {}", p.display()))?;

        let mut magic_buf = [0u8; 4];
        file.read_exact(&mut magic_buf)
            .with_context(|| format!("Failed to read magic header from {}", p.display()))?;

        let magic_be = u32::from_be_bytes(magic_buf);
        let magic_le = u32::from_le_bytes(magic_buf);

        if magic_be == Self::RAFS_SUPER_MAGIC || magic_le == Self::RAFS_SUPER_MAGIC {
            return Ok(AccelerationFormat::RafsV6);
        }
        if magic_be == Self::EROFS_SUPER_MAGIC_V1 || magic_le == Self::EROFS_SUPER_MAGIC_V1 {
            return Ok(AccelerationFormat::Erofs);
        }

        // Also check offset 1024 (standard EROFS superblock location)
        let mut full_header = vec![0u8; 1028];
        let mut file2 = File::open(p)?;
        if let Ok(n) = file2.read(&mut full_header) {
            if n >= 1028 {
                let erofs_magic = u32::from_le_bytes([
                    full_header[1024],
                    full_header[1025],
                    full_header[1026],
                    full_header[1027],
                ]);
                if erofs_magic == Self::EROFS_SUPER_MAGIC_V1 {
                    return Ok(AccelerationFormat::Erofs);
                }
            }
        }

        bail!(
            "File '{}' is not a recognized RAFSv6 or EROFS bootstrap metadata image",
            p.display()
        )
    }

    /// Calculates the number of chunks required for a file of `total_bytes` under the configured chunk size.
    pub fn calculate_chunk_count(&self, total_bytes: u64) -> u64 {
        let chunk_size = self.chunk_size_bytes.unwrap_or(4 * 1024 * 1024);
        if total_bytes == 0 || chunk_size == 0 {
            0
        } else {
            total_bytes.div_ceil(chunk_size)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_format_parsing() {
        assert_eq!(
            "rafsv6".parse::<AccelerationFormat>().unwrap(),
            AccelerationFormat::RafsV6
        );
        assert_eq!(
            "rafs".parse::<AccelerationFormat>().unwrap(),
            AccelerationFormat::RafsV6
        );
        assert_eq!(
            "nydus".parse::<AccelerationFormat>().unwrap(),
            AccelerationFormat::RafsV6
        );
        assert_eq!(
            "erofs".parse::<AccelerationFormat>().unwrap(),
            AccelerationFormat::Erofs
        );
        assert_eq!(
            "zran".parse::<AccelerationFormat>().unwrap(),
            AccelerationFormat::Zran
        );
        assert!("invalid".parse::<AccelerationFormat>().is_err());
    }

    #[test]
    fn test_chunk_count_calculation() {
        let acc = ImageAcceleration::default().with_chunk_size(4 * 1024 * 1024);
        assert_eq!(acc.calculate_chunk_count(0), 0);
        assert_eq!(acc.calculate_chunk_count(1024), 1);
        assert_eq!(acc.calculate_chunk_count(4 * 1024 * 1024), 1);
        assert_eq!(acc.calculate_chunk_count(4 * 1024 * 1024 + 1), 2);
        assert_eq!(acc.calculate_chunk_count(10 * 1024 * 1024), 3);

        // Macro-chunk test (64 MiB for LLM weights)
        let acc_llm = ImageAcceleration::default().with_chunk_size(64 * 1024 * 1024);
        // 1 GB model weights = 1024 MB / 64 MB = 16 macro-chunks
        assert_eq!(acc_llm.calculate_chunk_count(1024 * 1024 * 1024), 16);
    }

    #[test]
    fn test_inspect_rafs_magic() {
        let mut tmp = NamedTempFile::new().unwrap();
        // Write RAFS magic 0x52414653 ("RAFS")
        tmp.write_all(&ImageAcceleration::RAFS_SUPER_MAGIC.to_be_bytes())
            .unwrap();
        tmp.flush().unwrap();

        let fmt = ImageAcceleration::inspect_bootstrap_magic(tmp.path()).unwrap();
        assert_eq!(fmt, AccelerationFormat::RafsV6);
    }

    #[test]
    fn test_inspect_erofs_magic() {
        let mut tmp = NamedTempFile::new().unwrap();
        // Write 1024 dummy bytes, then EROFS magic
        let mut buf = vec![0u8; 1024];
        buf.extend_from_slice(&ImageAcceleration::EROFS_SUPER_MAGIC_V1.to_le_bytes());
        tmp.write_all(&buf).unwrap();
        tmp.flush().unwrap();

        let fmt = ImageAcceleration::inspect_bootstrap_magic(tmp.path()).unwrap();
        assert_eq!(fmt, AccelerationFormat::Erofs);
    }

    #[test]
    fn test_invalid_bootstrap_magic() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"NOT_A_VALID_MAGIC").unwrap();
        tmp.flush().unwrap();

        let res = ImageAcceleration::inspect_bootstrap_magic(tmp.path());
        assert!(res.is_err());
    }

    #[test]
    fn test_serialization() {
        let acc = ImageAcceleration::default()
            .with_format(AccelerationFormat::RafsV6)
            .with_lazy_load(true)
            .with_chunk_size(64 * 1024 * 1024)
            .with_prefetch(vec!["/usr/bin".to_string(), "/lib".to_string()]);

        let json = serde_json::to_string_pretty(&acc).unwrap();
        let de: ImageAcceleration = serde_json::from_str(&json).unwrap();
        assert_eq!(acc, de);
    }
}
