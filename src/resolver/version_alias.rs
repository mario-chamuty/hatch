/// Version aliasing for compatibility fixes
#[derive(Debug, Clone)]
pub struct VersionAlias {
    pub actual_version: String,
    pub pretend_version: String,
    pub is_path: bool,
}

impl VersionAlias {
    pub fn parse(alias_str: &str) -> Option<Self> {
        // Parse "1.8.3 as 1.9.9" or "dev-main as 2.0.0" or "path:../my-fork as 1.5.0"
        let parts: Vec<&str> = alias_str.split(" as ").collect();
        if parts.len() != 2 {
            return None;
        }

        let actual = parts[0].trim();
        let pretend = parts[1].trim();

        let is_path = actual.starts_with("path:");

        Some(VersionAlias {
            actual_version: actual.to_string(),
            pretend_version: pretend.to_string(),
            is_path,
        })
    }
}