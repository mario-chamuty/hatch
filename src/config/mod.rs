pub mod settings;
pub mod paths;

use anyhow::Result;
use std::path::PathBuf;

/// Configuration management for Hatch
pub struct ConfigManager {
    project_dir: PathBuf,
}

impl ConfigManager {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir }
    }
    
    /// Initialize configuration management
    pub fn init() -> Result<Self> {
        let project_dir = std::env::current_dir()?;
        Ok(Self::new(project_dir))
    }
    
    /// Get project directory
    pub fn project_dir(&self) -> &PathBuf {
        &self.project_dir
    }
}