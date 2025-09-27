pub mod git;
pub mod fs;
pub mod progress;
pub mod env;

/// Utility functions and helpers
pub struct Utilities;

impl Utilities {
    /// Check if we're in a git repository
    pub fn is_git_repo() -> bool {
        std::path::Path::new(".git").exists()
    }
    
    /// Get current directory name
    pub fn current_dir_name() -> Option<String> {
        std::env::current_dir()
            .ok()?
            .file_name()?
            .to_str()
            .map(|s| s.to_string())
    }
}