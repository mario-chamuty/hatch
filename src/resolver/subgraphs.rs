//! Independent-subgraph parallelism.
//!
//! After the top-down interval propagator has narrowed each package's
//! constraint down as far as it can, what remains may still split into
//! several disjoint dependency components. Running pubgrub independently on
//! each component gives us free parallelism – a single resolver call can
//! solve N components on N threads at once.
//!
//! Algorithm:
//! 1. Build an undirected graph whose nodes are package names. For every
//!    package that is still in the partial set, add an edge to each of its
//!    possible dependencies. "Possible" means: inspect every version
//!    currently permitted by the package's interval and take the union of
//!    their declared dependencies.
//! 2. Partition into connected components using `petgraph::algo::connected_components`.
//!    Components of size 1 are trivial – their version was already picked
//!    (or will be via the propagator's previous pass); no solver pass is
//!    required.
//! 3. For every component of size >= 2, spawn a pubgrub solve inside a rayon
//!    scope. Each worker gets its own tokio Handle scope so `HatchProvider`
//!    (which holds a non-Send `PubGrubError` in transit) can drive async
//!    fetches back onto the parent runtime.
//! 4. Merge all per-component solutions into a single `HashMap<name, ver>`.
//!    Disjoint components cannot disagree on a package, so a straight
//!    `extend` is sound.
//!
//! Rollback: `HATCH_NO_SUBGRAPH_PARALLEL=1` forces a single-component path
//! (i.e. hand everything straight to the single pubgrub solver).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use log::{debug, trace};
use petgraph::graph::{NodeIndex, UnGraph};

use crate::cache::metadata_cache::MetadataCache;
use crate::manifest::schema::SdkConstraints;
use crate::registry::traits::{ParsedConstraint, VersionConstraint};
use crate::resolver::error::ResolverError;
use crate::resolver::propagation::{is_sdk_pseudo_package, Overrides};
use crate::resolver::pubgrub_adapter;

/// Result returned from the scoped subgraph driver.
pub struct SubgraphSolution {
    /// Merged `package -> version` map across all components.
    pub resolved: HashMap<String, semver::Version>,
    /// How many components were solved in total (including size-1
    /// components that were forwarded to a trivial single-solver pass).
    pub components: usize,
    /// How many components actually needed pubgrub – i.e. size >= 2.
    pub non_trivial: usize,
}

fn parallel_disabled() -> bool {
    std::env::var("HATCH_NO_SUBGRAPH_PARALLEL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Compute the set of packages that the given package might depend on, by
/// unioning the declared dependencies of every version in its interval. If
/// the metadata cache has no entry (typical for the synthetic root), the
/// returned set is empty and that node simply has no outgoing edges – the
/// resulting component will be a singleton.
async fn possible_deps(
    cache: &MetadataCache,
    name: &str,
    interval: &ParsedConstraint,
) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(idx) = cache.get_index(name).await else {
        return out;
    };
    if idx.is_empty() {
        return out;
    }
    // Iterate every version in range via the bit-packed VersionSet so callers
    // that want to refine further can reuse the set algebra. Stream E phase 2
    // wired this up so `VersionSet::iter` / `VersionIndex::iter_versions`
    // are not dead code.
    let set = idx.matching(interval);
    for i in set.iter() {
        if let Some(pv) = idx.get(i) {
            for dep in pv.dependencies.keys() {
                if is_sdk_pseudo_package(dep) {
                    continue;
                }
                out.insert(dep.clone());
            }
        }
    }
    out
}

/// Partition `partial` into connected components. Each returned vec is the
/// list of package names in that component.
async fn build_components(
    cache: &MetadataCache,
    partial: &HashMap<String, ParsedConstraint>,
) -> Vec<Vec<String>> {
    let names: Vec<String> = partial.keys().cloned().collect();
    let mut graph: UnGraph<String, ()> = UnGraph::new_undirected();
    let mut name_to_node: HashMap<String, NodeIndex> = HashMap::new();

    for n in &names {
        let ix = graph.add_node(n.clone());
        name_to_node.insert(n.clone(), ix);
    }

    // Build the adjacency: for each package, union the dep names across all
    // permitted versions. Add edges for every (pkg, dep) pair where `dep`
    // is also in `partial` (we only care about connectivity WITHIN the
    // residual set; packages already resolved by propagation sit on the
    // sidelines).
    for n in &names {
        let Some(interval) = partial.get(n) else { continue };
        let deps = possible_deps(cache, n, interval).await;
        let Some(&src) = name_to_node.get(n) else { continue };
        for dep in deps {
            if let Some(&dst) = name_to_node.get(&dep) {
                if src != dst && !graph.contains_edge(src, dst) {
                    graph.add_edge(src, dst, ());
                }
            }
        }
    }

    // `petgraph::algo::connected_components` returns the COUNT; we need the
    // partition, so we do a plain BFS across the undirected graph ourselves.
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut components: Vec<Vec<String>> = Vec::new();

    for n in &names {
        let Some(&ix) = name_to_node.get(n) else { continue };
        if visited.contains(&ix) {
            continue;
        }
        let mut stack = vec![ix];
        let mut comp: Vec<String> = Vec::new();
        while let Some(cur) = stack.pop() {
            if !visited.insert(cur) {
                continue;
            }
            if let Some(name) = graph.node_weight(cur) {
                comp.push(name.clone());
            }
            for nb in graph.neighbors(cur) {
                if !visited.contains(&nb) {
                    stack.push(nb);
                }
            }
        }
        if !comp.is_empty() {
            components.push(comp);
        }
    }

    components
}

/// Solve the residual constraints (what propagation could not pin) by
/// decomposing them into independent components and solving each in its own
/// rayon thread.
///
/// Returns the merged solution map. Conflicts bubble up as
/// `ResolverError::NoSolution` – the first failing component wins (we don't
/// attempt to merge derivation trees from multiple components because
/// petgraph guarantees the conflicts are textually disjoint).
///
/// Single-component / tiny residuals: we fall through to
/// [`pubgrub_adapter::solve`] directly rather than pay the thread-spawn
/// cost.
pub async fn solve_components(
    cache: Arc<MetadataCache>,
    sdks: SdkConstraints,
    overrides: Overrides,
    locked: HashMap<String, semver::Version>,
    root_name: String,
    root_version: semver::Version,
    partial: HashMap<String, ParsedConstraint>,
) -> Result<SubgraphSolution, ResolverError> {
    if partial.is_empty() {
        return Ok(SubgraphSolution {
            resolved: HashMap::new(),
            components: 0,
            non_trivial: 0,
        });
    }

    if parallel_disabled() {
        debug!("Subgraph parallelism disabled by HATCH_NO_SUBGRAPH_PARALLEL");
        let out = pubgrub_adapter::solve(
            cache,
            sdks,
            overrides,
            locked,
            root_name,
            root_version,
            partial,
        )
        .await?;
        return Ok(SubgraphSolution {
            resolved: out,
            components: 1,
            non_trivial: 1,
        });
    }

    let components = build_components(&cache, &partial).await;
    let total_components = components.len();

    // Cheap path: one component. No need for rayon, spawn the adapter once.
    if components.len() <= 1 {
        let out = pubgrub_adapter::solve(
            cache,
            sdks,
            overrides,
            locked,
            root_name,
            root_version,
            partial,
        )
        .await?;
        return Ok(SubgraphSolution {
            resolved: out,
            components: total_components.max(1),
            non_trivial: if total_components <= 1 { 1 } else { 0 },
        });
    }

    trace!(
        "Subgraph solver: {} components from {} residual packages",
        components.len(),
        partial.len()
    );

    // Partition per-component deps/locked/overrides so each worker only
    // sees its own slice. Overrides and locked are global keys that apply
    // only to members of the component.
    let mut component_inputs: Vec<(
        Vec<String>,
        HashMap<String, ParsedConstraint>,
        HashMap<String, semver::Version>,
        Overrides,
    )> = Vec::with_capacity(components.len());

    for comp in &components {
        let set: HashSet<&String> = comp.iter().collect();
        let deps: HashMap<String, ParsedConstraint> = partial
            .iter()
            .filter(|(k, _)| set.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let lk: HashMap<String, semver::Version> = locked
            .iter()
            .filter(|(k, _)| set.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let ov: Overrides = overrides
            .iter()
            .filter(|(k, _)| set.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        component_inputs.push((comp.clone(), deps, lk, ov));
    }

    let handle = tokio::runtime::Handle::current();
    let cache_arc = cache.clone();
    let sdks_shared = sdks.clone();
    let root_name_shared = root_name.clone();
    let root_ver_shared = root_version.clone();

    // Run rayon inside spawn_blocking so we never block a tokio worker
    // thread with the sync fan-out. Each rayon thread drives pubgrub for
    // one component using its own `block_on` against the shared handle.
    let results = tokio::task::spawn_blocking(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(component_inputs.len().min(8))
            .build();
        let pool = match pool {
            Ok(p) => p,
            Err(e) => {
                return Err(ResolverError::Io(format!("rayon build: {e}")));
            }
        };

        pool.scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel();
            for (comp_idx, (comp, deps, lk, ov)) in component_inputs.into_iter().enumerate() {
                let tx = tx.clone();
                let handle = handle.clone();
                let cache_arc = cache_arc.clone();
                let sdks_shared = sdks_shared.clone();
                let root_name_shared = root_name_shared.clone();
                let root_ver_shared = root_ver_shared.clone();

                scope.spawn(move |_| {
                    // Uniquify the synthetic root per component so the
                    // metadata cache entries don't clobber each other.
                    let comp_root = format!("{}__comp_{}", root_name_shared, comp_idx);

                    let result = handle.block_on(pubgrub_adapter::solve(
                        cache_arc,
                        sdks_shared,
                        ov,
                        lk,
                        comp_root,
                        root_ver_shared,
                        deps,
                    ));
                    let _ = tx.send((comp, result));
                });
            }
            drop(tx);

            let mut merged: HashMap<String, semver::Version> = HashMap::new();
            let mut err: Option<ResolverError> = None;
            for (_comp, res) in rx.iter() {
                match res {
                    Ok(map) => merged.extend(map),
                    Err(e) => {
                        // First error wins. Continue draining the channel
                        // so no sender is left dangling, but don't
                        // overwrite.
                        if err.is_none() {
                            err = Some(e);
                        }
                    }
                }
            }
            match err {
                Some(e) => Err(e),
                None => Ok(merged),
            }
        })
    })
    .await
    .map_err(|e| ResolverError::Io(format!("subgraph join failed: {e}")))??;

    let non_trivial = components.iter().filter(|c| c.len() >= 2).count();
    Ok(SubgraphSolution {
        resolved: results,
        components: total_components,
        non_trivial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::traits::PackageVersion;

    fn pv(version: &str, deps: &[(&str, &str)]) -> PackageVersion {
        PackageVersion {
            version: version.to_string(),
            description: None,
            homepage: None,
            repository: None,
            dependencies: deps
                .iter()
                .map(|(n, c)| (n.to_string(), c.to_string()))
                .collect(),
            dev_dependencies: HashMap::new(),
            published: None,
            dart_sdk: None,
            flutter_sdk: None,
            archive_sha256: None,
        }
    }

    fn c(constraint: &str) -> ParsedConstraint {
        VersionConstraint::parse(constraint)
            .and_then(|x| x.to_parsed())
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disjoint_components_are_split() {
        let cache = MetadataCache::new();
        cache
            .insert("alpha".into(), vec![pv("1.0.0", &[])])
            .await;
        cache
            .insert("beta".into(), vec![pv("2.0.0", &[])])
            .await;

        let mut partial: HashMap<String, ParsedConstraint> = HashMap::new();
        partial.insert("alpha".into(), c("^1.0.0"));
        partial.insert("beta".into(), c("^2.0.0"));

        let comps = build_components(&cache, &partial).await;
        assert_eq!(comps.len(), 2, "expected two disjoint components");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_components_are_merged() {
        let cache = MetadataCache::new();
        cache
            .insert("alpha".into(), vec![pv("1.0.0", &[("beta", "^2.0.0")])])
            .await;
        cache
            .insert("beta".into(), vec![pv("2.0.0", &[])])
            .await;

        let mut partial: HashMap<String, ParsedConstraint> = HashMap::new();
        partial.insert("alpha".into(), c("^1.0.0"));
        partial.insert("beta".into(), c("^2.0.0"));

        let comps = build_components(&cache, &partial).await;
        assert_eq!(comps.len(), 1, "alpha->beta should collapse to one");
    }
}
