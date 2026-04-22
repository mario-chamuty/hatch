pub mod graph;
pub mod version;
pub mod cache;
pub mod ultra;
pub mod dependency_utils;
pub mod version_alias;
pub mod version_index;
pub mod error;
pub mod propagation;
pub mod pubgrub_adapter;
pub mod resolution_cache;
pub mod subgraphs;

pub struct DependencyResolver {
    // TODO: Add resolver state
}

impl DependencyResolver {
    pub fn new() -> Self {
        Self {}
    }
}