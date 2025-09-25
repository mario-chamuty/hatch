// CLI argument type definitions and utilities

use clap::{ArgMatches, Command};

/// Utility functions for argument validation and processing
pub mod validation {
    use anyhow::{anyhow, Result};
    use regex::Regex;
    
    /// Validate package name format
    pub fn validate_package_name(name: &str) -> Result<()> {
        let re = Regex::new(r"^[a-z][a-z0-9_]*$")?;
        if re.is_match(name) {
            Ok(())
        } else {
            Err(anyhow!("Invalid package name: {}", name))
        }
    }
    
    /// Validate version constraint format
    pub fn validate_version_constraint(constraint: &str) -> Result<()> {
        // Basic validation - should be enhanced with proper semver parsing
        if constraint.is_empty() {
            Err(anyhow!("Version constraint cannot be empty"))
        } else {
            Ok(())
        }
    }
}

/// Common argument patterns and defaults
pub mod defaults {
    pub const DEFAULT_PROFILE: &str = "default";
    pub const DEFAULT_CHANNEL: &str = "dev";
    pub const DEFAULT_BUILD_PROFILE: &str = "release";
}