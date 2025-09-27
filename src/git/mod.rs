use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use log::{debug, info};

pub struct GitResolver;

impl GitResolver {
    /// Clone a git repository to the cache
    pub fn clone_repository(
        url: &str,
        git_ref: Option<&str>,
        target_dir: &Path,
    ) -> Result<()> {
        info!("Cloning git repository: {}", url);

        // Ensure target directory's parent exists
        if let Some(parent) = target_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Clone the repository
        let mut cmd = Command::new("git");
        cmd.arg("clone")
           .arg("--depth").arg("1");

        if let Some(ref_name) = git_ref {
            cmd.arg("--branch").arg(ref_name);
        }

        cmd.arg(url)
           .arg(target_dir);

        debug!("Running git command: {:?}", cmd);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to clone repository: {}", stderr));
        }

        Ok(())
    }

    /// Check if a directory is a git repository with the expected remote
    pub fn is_valid_git_repo(dir: &Path, expected_url: &str) -> bool {
        if !dir.join(".git").exists() {
            return false;
        }

        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .arg("remote")
            .arg("get-url")
            .arg("origin")
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let remote_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                remote_url == expected_url || remote_url == expected_url.replace("https://", "git@").replace("/", ":")
            }
            _ => false
        }
    }

    /// Get the git package cache directory
    pub fn get_cache_dir(url: &str, git_ref: Option<&str>) -> Result<PathBuf> {
        let cache_root = crate::cache::paths::CachePaths::root()?;
        let git_cache = cache_root.join("git");

        // Create a safe directory name from the URL
        let safe_name = url.replace("https://", "")
            .replace("http://", "")
            .replace("git@", "")
            .replace("/", "_")
            .replace(":", "_")
            .replace(".git", "");

        let mut dir = git_cache.join(&safe_name);

        if let Some(ref_name) = git_ref {
            dir = dir.join(ref_name);
        } else {
            dir = dir.join("default");
        }

        Ok(dir)
    }
}