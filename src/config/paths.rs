use anyhow::Result;
use dirs::home_dir;
use std::path::PathBuf;

/// Path resolution utilities
pub struct PathResolver {
    project_dir: PathBuf,
}

impl PathResolver {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir }
    }
    
    /// Get Hatch cache directory
    pub fn cache_dir() -> Result<PathBuf> {
        let home = home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
        Ok(home.join(".hatch").join("cache"))
    }
    
    /// Get hatch.yaml manifest path
    pub fn manifest_yaml_path(&self) -> PathBuf {
        self.project_dir.join("hatch.yaml")
    }
    
    /// Get hatch.json manifest path
    pub fn manifest_json_path(&self) -> PathBuf {
        self.project_dir.join("hatch.json")
    }
    
    /// Get local override manifest path (YAML)
    pub fn local_manifest_yaml_path(&self) -> PathBuf {
        self.project_dir.join("hatch.local.yaml")
    }
    
    /// Get local override manifest path (JSON)
    pub fn local_manifest_json_path(&self) -> PathBuf {
        self.project_dir.join("hatch.local.json")
    }
    
    /// Get pubspec.yaml path
    pub fn pubspec_path(&self) -> PathBuf {
        self.project_dir.join("pubspec.yaml")
    }
    
    /// Get pubspec.lock path
    pub fn pubspec_lock_path(&self) -> PathBuf {
        self.project_dir.join("pubspec.lock")
    }
    
    /// Find manifest file (prefer YAML over JSON)
    pub fn find_manifest(&self) -> Option<PathBuf> {
        let yaml_path = self.manifest_yaml_path();
        let json_path = self.manifest_json_path();
        
        if yaml_path.exists() {
            Some(yaml_path)
        } else if json_path.exists() {
            Some(json_path)
        } else {
            None
        }
    }
}