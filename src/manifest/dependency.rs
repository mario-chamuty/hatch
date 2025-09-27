use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

/// Represents a dependency that can be either a simple version string
/// or a complex object with version, path, git, or nest source
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    /// Simple version constraint string (e.g., "^1.0.0")
    Simple(String),
    /// Complex dependency with source information
    Complex(ComplexDependency),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexDependency {
    #[serde(default = "default_version")]
    pub version: String,

    /// Path to local package
    pub path: Option<String>,

    /// Git repository URL
    pub git: Option<String>,

    /// Git branch/tag/commit
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,

    /// Nest registry name
    pub nest: Option<String>,

    /// SDK source (e.g., "flutter")
    pub sdk: Option<String>,
}

fn default_version() -> String {
    "any".to_string()
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Dependency::Simple(v) => write!(f, "{}", v),
            Dependency::Complex(c) => {
                if c.path.is_some() {
                    write!(f, "{} (path)", c.version)
                } else if c.git.is_some() {
                    write!(f, "{} (git)", c.version)
                } else if c.nest.is_some() {
                    write!(f, "{} (nest: {})", c.version, c.nest.as_ref().unwrap())
                } else if c.sdk.is_some() {
                    write!(f, "SDK: {}", c.sdk.as_ref().unwrap())
                } else {
                    write!(f, "{}", c.version)
                }
            }
        }
    }
}

impl Dependency {
    /// Get the version constraint
    pub fn version(&self) -> &str {
        match self {
            Dependency::Simple(v) => v,
            Dependency::Complex(c) => &c.version,
        }
    }

    /// Check if this is a local path dependency
    pub fn is_local(&self) -> bool {
        match self {
            Dependency::Simple(_) => false,
            Dependency::Complex(c) => c.path.is_some(),
        }
    }

    /// Get the local path if this is a path dependency
    pub fn local_path(&self) -> Option<&str> {
        match self {
            Dependency::Simple(_) => None,
            Dependency::Complex(c) => c.path.as_deref(),
        }
    }

    /// Check if this is a git dependency
    pub fn is_git(&self) -> bool {
        match self {
            Dependency::Simple(_) => false,
            Dependency::Complex(c) => c.git.is_some(),
        }
    }

    /// Check if this is from a specific nest
    pub fn is_from_nest(&self) -> bool {
        match self {
            Dependency::Simple(_) => false,
            Dependency::Complex(c) => c.nest.is_some(),
        }
    }

    /// Get the nest name if specified
    pub fn nest_name(&self) -> Option<&str> {
        match self {
            Dependency::Simple(_) => None,
            Dependency::Complex(c) => c.nest.as_deref(),
        }
    }

    /// Check if this is an SDK dependency
    pub fn is_sdk(&self) -> bool {
        match self {
            Dependency::Simple(_) => false,
            Dependency::Complex(c) => c.sdk.is_some(),
        }
    }

    /// Validate the dependency for conflicting sources
    pub fn validate(&self, name: &str) -> Result<(), String> {
        match self {
            Dependency::Simple(_) => Ok(()), // Simple dependencies are always valid
            Dependency::Complex(c) => c.validate(name),
        }
    }
}

impl ComplexDependency {
    /// Validate that only one source type is specified
    pub fn validate(&self, name: &str) -> Result<(), String> {
        let mut sources = Vec::new();

        if self.path.is_some() {
            sources.push("path");
        }
        if self.git.is_some() {
            sources.push("git");
        }
        if self.nest.is_some() {
            sources.push("nest");
        }
        if self.sdk.is_some() {
            sources.push("sdk");
        }

        if sources.len() > 1 {
            return Err(format!(
                "Dependency '{}' has conflicting sources: {}. Only one source type is allowed per dependency",
                name,
                sources.join(" and ")
            ));
        }

        // Validate that git_ref is only used with git source
        if self.git_ref.is_some() && self.git.is_none() {
            return Err(format!(
                "Dependency '{}' specifies 'ref' without a 'git' source",
                name
            ));
        }

        Ok(())
    }

    /// Validate that referenced nest exists
    pub fn validate_nest(&self, name: &str, available_nests: &[String]) -> Result<(), String> {
        if let Some(nest_name) = &self.nest {
            if !available_nests.contains(nest_name) {
                return Err(format!(
                    "Dependency '{}' references undefined nest '{}'. Available nests: {}",
                    name,
                    nest_name,
                    if available_nests.is_empty() {
                        "none".to_string()
                    } else {
                        available_nests.join(", ")
                    }
                ));
            }
        }
        Ok(())
    }
}

/// Parse dependencies from JSON value
pub fn parse_dependencies(value: &Value) -> HashMap<String, Dependency> {
    let mut deps = HashMap::new();

    if let Some(obj) = value.as_object() {
        for (name, dep_value) in obj {
            if let Ok(dep) = serde_json::from_value::<Dependency>(dep_value.clone()) {
                deps.insert(name.clone(), dep);
            }
        }
    }

    deps
}