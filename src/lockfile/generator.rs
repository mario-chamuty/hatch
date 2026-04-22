use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use log::info;

use super::parser::{HatchLockfile, LockedPackage};
use crate::manifest::schema::HatchManifest;
use crate::resolver::graph::ResolutionGraph;

pub struct LockfileGenerator;

impl LockfileGenerator {
    /// Generate a lockfile from a finished resolution graph.
    pub fn generate_from_graph(
        manifest: &HatchManifest,
        graph: &ResolutionGraph,
        path: &Path,
    ) -> Result<()> {
        info!(
            "Generating lockfile with {} packages",
            graph.resolved.len()
        );

        let mut packages = HashMap::new();
        for (name, version) in &graph.resolved {
            if name == "flutter" {
                continue;
            }
            let deps = graph
                .deps
                .get(name)
                .cloned()
                .unwrap_or_default();
            let locked = LockedPackage {
                version: version.clone(),
                resolved: format!("pub.dev/{}@{}", name, version),
                integrity: None,
                dependencies: deps,
                dev: false,
                registry: "pub.dev".to_string(),
            };
            packages.insert(name.clone(), locked);
        }

        let lockfile = HatchLockfile {
            lockfile_version: 1,
            packages,
            flutter_version: manifest.sdk.flutter.clone(),
            dart_version: manifest.sdk.dart.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            hatch_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let yaml = serde_yaml::to_string(&lockfile)?;
        std::fs::write(path, yaml)?;
        info!("Lockfile written to {}", path.display());
        Ok(())
    }

    /// Back-compat wrapper: accept a simple `HashMap<name, version>` and an
    /// optional per-package dependency map. Used by call sites that don't
    /// have a full graph yet.
    pub fn generate_from_resolved(
        manifest: &HatchManifest,
        resolved: &HashMap<String, String>,
        deps_of: &HashMap<String, HashMap<String, String>>,
        path: &Path,
    ) -> Result<()> {
        let graph = ResolutionGraph {
            resolved: resolved.clone(),
            deps: deps_of.clone(),
            resolved_paths: HashMap::new(),
        };
        Self::generate_from_graph(manifest, &graph, path)
    }

    pub fn is_up_to_date(_manifest: &HatchManifest, lockfile_path: &Path) -> bool {
        if !lockfile_path.exists() {
            return false;
        }
        super::parser::LockfileParser::parse(lockfile_path).is_ok()
    }
}
