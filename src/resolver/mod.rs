pub mod sat;
pub mod graph;
pub mod version;
pub mod cache;
pub mod ultra;
pub mod dependency_utils;
pub mod version_alias;

pub struct DependencyResolver {
    // TODO: Add resolver state
}

impl DependencyResolver {
    pub fn new() -> Self {
        Self {}
    }
}