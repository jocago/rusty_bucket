use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RateLimit {
    pub enabled: bool,
    pub bytes_per_second: Option<u64>,     // Bytes per second
    pub megabytes_per_minute: Option<u64>, // Megabytes per minute
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            enabled: false,
            bytes_per_second: None,
            megabytes_per_minute: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileOperation {
    pub name: String,
    pub origin: PathBuf,
    pub destination: PathBuf,
    pub operation_type: OperationType,
    pub rate_limit: RateLimit, // NEW: Rate limiting per operation
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum OperationType {
    Copy,
    Move,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub operations: Vec<FileOperation>,
    pub global_rate_limit: RateLimit, // NEW: Global rate limit
}

impl Config {
    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn save_to_file(&self, path: &str) -> anyhow::Result<()> {
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
