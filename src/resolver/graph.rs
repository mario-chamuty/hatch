use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use log::{debug, info, warn};

use crate::registry::traits::{VersionConstraint, PackageVersion, DependencySource};

/// Dependency graph node
#[derive(Debug, Clone)]
pub struct DependencyNode {
    pub name: String,
    pub version: String,
    pub source: DependencySource,
    pub dependencies: HashMap<String, String>,
    pub dev_dependencies: HashMap<String, String>,
}

/// Dependency graph for resolution
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    pub nodes: HashMap<String, DependencyNode>,
    pub edges: HashMap<String, Vec<String>>,
}

/// Dependency conflict information
#[derive(Debug, Clone)]
pub struct DependencyConflict {
    pub package: String,
    pub requested_versions: Vec<VersionRequest>,
}

#[derive(Debug, Clone)]
pub struct VersionRequest {
    pub version: String,
    pub constraint: String,
    pub requester: String,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
        }
    }

    /// Add a dependency node to the graph
    pub fn add_node(&mut self, node: DependencyNode) {
        debug!("Adding node: {}@{}", node.name, node.version);

        // Add edges for dependencies
        let mut deps = Vec::new();
        for dep_name in node.dependencies.keys() {
            deps.push(dep_name.clone());
        }

        self.edges.insert(node.name.clone(), deps);
        self.nodes.insert(node.name.clone(), node);
    }

    /// Check for circular dependencies
    pub fn check_circular_dependencies(&self) -> Result<Vec<Vec<String>>> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut cycles = Vec::new();

        for node_name in self.nodes.keys() {
            if !visited.contains(node_name) {
                if let Some(cycle) = self.dfs_cycle_detection(
                    node_name,
                    &mut visited,
                    &mut rec_stack
                ) {
                    cycles.push(cycle);
                }
            }
        }

        Ok(cycles)
    }

    fn dfs_cycle_detection(
        &self,
        node: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
    ) -> Option<Vec<String>> {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());

        if let Some(edges) = self.edges.get(node) {
            for neighbor in edges {
                if !visited.contains(neighbor) {
                    if let Some(cycle) = self.dfs_cycle_detection(neighbor, visited, rec_stack) {
                        let mut full_cycle = vec![node.to_string()];
                        full_cycle.extend(cycle);
                        return Some(full_cycle);
                    }
                } else if rec_stack.contains(neighbor) {
                    return Some(vec![node.to_string(), neighbor.to_string()]);
                }
            }
        }

        rec_stack.remove(node);
        None
    }

    /// Get topological sort of dependencies
    pub fn topological_sort(&self) -> Result<Vec<String>> {
        let mut in_degree = HashMap::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();

        // Initialize in-degree count
        for node_name in self.nodes.keys() {
            in_degree.insert(node_name.clone(), 0);
        }

        // Calculate in-degrees
        for edges in self.edges.values() {
            for target in edges {
                if let Some(count) = in_degree.get_mut(target) {
                    *count += 1;
                }
            }
        }

        // Add nodes with no incoming edges
        for (node_name, degree) in &in_degree {
            if *degree == 0 {
                queue.push_back(node_name.clone());
            }
        }

        // Process nodes
        while let Some(node) = queue.pop_front() {
            result.push(node.clone());

            if let Some(edges) = self.edges.get(&node) {
                for neighbor in edges {
                    if let Some(degree) = in_degree.get_mut(neighbor) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            return Err(anyhow!("Circular dependency detected"));
        }

        Ok(result)
    }

    /// Find conflicts in version requirements
    pub fn find_version_conflicts(&self, requirements: &HashMap<String, Vec<VersionRequest>>) -> Vec<DependencyConflict> {
        let mut conflicts = Vec::new();

        for (package, requests) in requirements {
            if requests.len() > 1 {
                // Check if all version constraints are compatible
                let mut compatible = true;

                for i in 0..requests.len() {
                    for j in i+1..requests.len() {
                        let constraint1 = VersionConstraint::parse(&requests[i].constraint).unwrap_or(VersionConstraint::Any);
                        let constraint2 = VersionConstraint::parse(&requests[j].constraint).unwrap_or(VersionConstraint::Any);

                        if !self.are_constraints_compatible(&constraint1, &constraint2) {
                            compatible = false;
                            break;
                        }
                    }
                    if !compatible {
                        break;
                    }
                }

                if !compatible {
                    conflicts.push(DependencyConflict {
                        package: package.clone(),
                        requested_versions: requests.clone(),
                    });
                }
            }
        }

        conflicts
    }

    fn are_constraints_compatible(&self, c1: &VersionConstraint, c2: &VersionConstraint) -> bool {
        use VersionConstraint::*;

        match (c1, c2) {
            (Any, _) | (_, Any) => true,
            (Exact(v1), Exact(v2)) => v1 == v2,
            (Exact(v), constraint) | (constraint, Exact(v)) => constraint.satisfies(v),
            _ => {
                // For complex constraints, we'd need a more sophisticated check
                // For now, assume they're compatible if they're not obviously conflicting
                true
            }
        }
    }

    /// Get all transitive dependencies for a package
    pub fn get_transitive_dependencies(&self, package: &str) -> HashSet<String> {
        let mut visited = HashSet::new();
        let mut to_visit = VecDeque::new();

        to_visit.push_back(package.to_string());

        while let Some(current) = to_visit.pop_front() {
            if visited.contains(&current) {
                continue;
            }

            visited.insert(current.clone());

            if let Some(edges) = self.edges.get(&current) {
                for dep in edges {
                    if !visited.contains(dep) {
                        to_visit.push_back(dep.clone());
                    }
                }
            }
        }

        // Remove the original package from the result
        visited.remove(package);
        visited
    }

    /// Find why a package is included (dependency path)
    pub fn find_dependency_path(&self, target: &str, roots: &[String]) -> Option<Vec<String>> {
        for root in roots {
            if let Some(path) = self.find_path_bfs(root, target) {
                return Some(path);
            }
        }
        None
    }

    fn find_path_bfs(&self, start: &str, target: &str) -> Option<Vec<String>> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut parent: HashMap<String, String> = HashMap::new();

        queue.push_back(start.to_string());
        visited.insert(start.to_string());

        while let Some(current) = queue.pop_front() {
            if current == target {
                // Reconstruct path
                let mut path = Vec::new();
                let mut node = target.to_string();

                while let Some(p) = parent.get(&node) {
                    path.push(node.clone());
                    node = p.clone();
                }
                path.push(start.to_string());
                path.reverse();

                return Some(path);
            }

            if let Some(edges) = self.edges.get(&current) {
                for neighbor in edges {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        parent.insert(neighbor.clone(), current.clone());
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        None
    }

    /// Get statistics about the dependency graph
    pub fn get_statistics(&self) -> DependencyGraphStats {
        let node_count = self.nodes.len();
        let edge_count: usize = self.edges.values().map(|v| v.len()).sum();

        let max_depth = self.calculate_max_depth();
        let cycles = self.check_circular_dependencies().unwrap_or_default();

        DependencyGraphStats {
            node_count,
            edge_count,
            max_depth,
            cycle_count: cycles.len(),
        }
    }

    fn calculate_max_depth(&self) -> usize {
        let mut max_depth = 0;

        for node in self.nodes.keys() {
            let depth = self.calculate_depth_from_node(node, &mut HashSet::new());
            max_depth = max_depth.max(depth);
        }

        max_depth
    }

    fn calculate_depth_from_node(&self, node: &str, visited: &mut HashSet<String>) -> usize {
        if visited.contains(node) {
            return 0; // Avoid infinite loops
        }

        visited.insert(node.to_string());

        let max_child_depth = self.edges.get(node)
            .map(|edges| {
                edges.iter()
                    .map(|child| self.calculate_depth_from_node(child, visited))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        visited.remove(node);
        max_child_depth + 1
    }
}

#[derive(Debug, Clone)]
pub struct DependencyGraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub max_depth: usize,
    pub cycle_count: usize,
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}