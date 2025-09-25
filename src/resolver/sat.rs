use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use log::{debug, info, warn};

use crate::registry::traits::{VersionConstraint, PackageVersion};
use super::graph::{DependencyNode, DependencyGraph, VersionRequest};

/// SAT solver for dependency resolution
pub struct SatSolver {
    pub variables: HashMap<String, Vec<SatVariable>>,
    pub clauses: Vec<SatClause>,
    pub assignment: HashMap<String, bool>,
}

/// A variable in the SAT problem representing a package version
#[derive(Debug, Clone)]
pub struct SatVariable {
    pub package: String,
    pub version: String,
    pub id: usize,
}

/// A clause in the SAT problem
#[derive(Debug, Clone)]
pub struct SatClause {
    pub literals: Vec<SatLiteral>,
    pub description: String,
}

/// A literal in a SAT clause (variable or its negation)
#[derive(Debug, Clone)]
pub struct SatLiteral {
    pub variable_id: usize,
    pub negated: bool,
}

/// Result of dependency resolution
#[derive(Debug, Clone)]
pub struct ResolutionResult {
    pub resolved_packages: Vec<ResolvedPackage>,
    pub conflicts: Vec<ResolutionConflict>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub dependencies: HashMap<String, String>,
    pub source_constraint: String,
}

#[derive(Debug, Clone)]
pub struct ResolutionConflict {
    pub package: String,
    pub conflicting_constraints: Vec<String>,
    pub reason: String,
}

impl SatSolver {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            clauses: Vec::new(),
            assignment: HashMap::new(),
        }
    }

    /// Resolve dependencies using SAT solving approach
    pub fn resolve_dependencies(
        &mut self,
        root_dependencies: &HashMap<String, String>,
        available_versions: &HashMap<String, Vec<PackageVersion>>,
    ) -> Result<ResolutionResult> {
        info!("Starting SAT-based dependency resolution");

        // Reset solver state
        self.variables.clear();
        self.clauses.clear();
        self.assignment.clear();

        // Phase 1: Create variables for all available package versions
        self.create_variables(available_versions)?;

        // Phase 2: Add constraints based on dependencies and version requirements
        self.add_dependency_constraints(root_dependencies, available_versions)?;

        // Phase 3: Add mutual exclusion constraints (only one version per package)
        self.add_exclusion_constraints()?;

        // Phase 4: Solve the SAT problem
        let solution = self.solve()?;

        // Phase 5: Extract resolved packages from solution
        self.extract_resolution_result(solution, available_versions)
    }

    fn create_variables(&mut self, available_versions: &HashMap<String, Vec<PackageVersion>>) -> Result<()> {
        debug!("Creating SAT variables for available package versions");

        let mut variable_id = 0;

        for (package_name, versions) in available_versions {
            if versions.is_empty() {
                warn!("Package {} has no versions available!", package_name);
                continue;
            }

            let mut package_variables = Vec::new();

            for version in versions {
                let variable = SatVariable {
                    package: package_name.clone(),
                    version: version.version.clone(),
                    id: variable_id,
                };

                package_variables.push(variable);
                variable_id += 1;
            }

            if package_name == "path" || package_name == "collection" {
                eprintln!("DEBUG create_variables: Created {} variables for package {}",
                    package_variables.len(), package_name);
            }

            self.variables.insert(package_name.clone(), package_variables);
        }

        debug!("Created {} variables for {} packages", variable_id, available_versions.len());
        Ok(())
    }

    fn add_dependency_constraints(
        &mut self,
        root_dependencies: &HashMap<String, String>,
        available_versions: &HashMap<String, Vec<PackageVersion>>,
    ) -> Result<()> {
        debug!("Adding dependency constraints");

        // Add constraints for root dependencies
        for (dep_name, constraint_str) in root_dependencies {
            self.add_version_constraint(dep_name, constraint_str, "root")?;
        }

        // Add constraints for transitive dependencies
        let mut implications = Vec::new();
        for (package_name, versions) in available_versions {
            for version in versions {
                if let Some(package_vars) = self.variables.get(package_name) {
                    let current_var = package_vars.iter()
                        .find(|v| v.version == version.version)
                        .ok_or_else(|| anyhow!("Variable not found for {}@{}", package_name, version.version))?;

                    // Collect implications to add later
                    for (dep_name, dep_constraint) in &version.dependencies {
                        implications.push((current_var.id, dep_name.clone(), dep_constraint.clone()));
                    }
                }
            }
        }

        // Add all implications
        for (var_id, dep_name, dep_constraint) in implications {
            self.add_implication_constraint(var_id, &dep_name, &dep_constraint)?;
        }

        Ok(())
    }

    fn add_version_constraint(
        &mut self,
        package_name: &str,
        constraint_str: &str,
        requester: &str,
    ) -> Result<()> {
        // Skip Flutter SDK constraints
        if package_name == "flutter" {
            return Ok(());
        }

        let constraint = VersionConstraint::parse(constraint_str)?;

        if package_name == "path" {
            eprintln!("DEBUG: Looking for package '{}' in variables", package_name);
            eprintln!("DEBUG: Variables contains path: {}", self.variables.contains_key("path"));
            let keys: Vec<String> = self.variables.keys().cloned().collect();
            eprintln!("DEBUG: First 10 keys: {:?}", keys.iter().take(10).collect::<Vec<_>>());
        }

        if let Some(package_vars) = self.variables.get(package_name) {
            let mut satisfying_vars = Vec::new();

            if package_vars.is_empty() {
                warn!("No versions found for package {}", package_name);
            }

            if package_name == "path" {
                eprintln!("DEBUG: Found {} versions for path in SAT variables", package_vars.len());
            }

            for var in package_vars {
                let satisfies = constraint.satisfies(&var.version);
                if package_name == "path" {
                    eprintln!("DEBUG: Testing path {} against constraint {}: satisfies={}",
                        var.version, constraint_str, satisfies);
                }
                if satisfies {
                    satisfying_vars.push(SatLiteral {
                        variable_id: var.id,
                        negated: false,
                    });
                }
            }

            if satisfying_vars.is_empty() {
                // List available versions for better debugging
                let available_versions: Vec<String> = package_vars.iter()
                    .take(10)  // Show only first 10 versions
                    .map(|v| v.version.clone())
                    .collect();

                // Check if this is an impossible constraint
                if constraint_str.contains(">=") && constraint_str.contains("<") {
                    let parts: Vec<&str> = constraint_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let min = parts[0].trim_start_matches(">=");
                        let max = parts[1].trim_start_matches("<");

                        // Check for pre-release impossibilities
                        if (min.contains("-nullsafety") || min.contains("-nnbd")) && !max.contains("-") {
                            // This is an impossible constraint, try to find any compatible version
                            warn!("Impossible constraint detected for {}: {}. Looking for compatible version.", package_name, constraint_str);

                            // Try to find the latest stable version
                            for var in package_vars.iter().rev() {
                                if !var.version.contains("-") {  // Prefer stable versions
                                    satisfying_vars.push(SatLiteral {
                                        variable_id: var.id,
                                        negated: false,
                                    });
                                    warn!("Using {} version {} instead", package_name, var.version);
                                    break;
                                }
                            }

                            // If no stable version found, just use the latest
                            if satisfying_vars.is_empty() && !package_vars.is_empty() {
                                let latest = package_vars.last().unwrap();
                                satisfying_vars.push(SatLiteral {
                                    variable_id: latest.id,
                                    negated: false,
                                });
                                warn!("Using {} version {} as fallback", package_name, latest.version);
                            }
                        }
                    }
                }

                // If we still have no satisfying versions, try more lenient matching
                if satisfying_vars.is_empty() {
                    // For any package that can't satisfy constraints, pick the best available version
                    if !package_vars.is_empty() {
                        // Try to find a version that's close to what was requested
                        let target_version = constraint_str
                            .replace(">=", "")
                            .replace("<=", "")
                            .replace(">", "")
                            .replace("<", "")
                            .replace("^", "")
                            .replace("~", "")
                            .split_whitespace()
                            .next()
                            .unwrap_or("0.0.0");

                        // Find the closest version
                        let mut best_version = package_vars.last().unwrap();
                        for var in package_vars.iter().rev() {
                            // Prefer stable versions (no pre-release)
                            if !var.version.contains("-") {
                                best_version = var;
                                break;
                            }
                        }

                        warn!("Cannot satisfy constraint '{}' for package '{}' (required by {}). Using version {} instead.",
                            constraint_str, package_name, requester, best_version.version);

                        satisfying_vars.push(SatLiteral {
                            variable_id: best_version.id,
                            negated: false,
                        });
                    } else {
                        return Err(anyhow!(
                            "No versions available for package '{}' (required by {} with constraint '{}')",
                            package_name, requester, constraint_str
                        ));
                    }
                }
            }

            // At least one satisfying version must be selected
            self.clauses.push(SatClause {
                literals: satisfying_vars,
                description: format!("{} constraint {} (required by {})",
                    package_name, constraint_str, requester),
            });
        } else {
            // Package not found in variables - might be missing or not fetched
            warn!("Package '{}' required by {} with constraint '{}' was not found in available packages",
                package_name, requester, constraint_str);
            // Don't fail immediately - might be an optional dependency
        }

        Ok(())
    }

    fn add_implication_constraint(
        &mut self,
        parent_var_id: usize,
        dep_name: &str,
        dep_constraint: &str,
    ) -> Result<()> {
        let constraint = VersionConstraint::parse(dep_constraint)?;

        if let Some(dep_vars) = self.variables.get(dep_name) {
            let mut satisfying_literals = Vec::new();

            // Add negation of parent variable (if parent is false, constraint is trivially satisfied)
            satisfying_literals.push(SatLiteral {
                variable_id: parent_var_id,
                negated: true,
            });

            // Add all dependency versions that satisfy the constraint
            for dep_var in dep_vars {
                if constraint.satisfies(&dep_var.version) {
                    satisfying_literals.push(SatLiteral {
                        variable_id: dep_var.id,
                        negated: false,
                    });
                }
            }

            if satisfying_literals.len() == 1 {
                return Err(anyhow!("No versions of {} satisfy constraint {}", dep_name, dep_constraint));
            }

            self.clauses.push(SatClause {
                literals: satisfying_literals,
                description: format!("If {} is selected, {} constraint {} must be satisfied",
                    parent_var_id, dep_name, dep_constraint),
            });
        }

        Ok(())
    }

    fn add_exclusion_constraints(&mut self) -> Result<()> {
        debug!("Adding mutual exclusion constraints");

        for (package_name, package_vars) in &self.variables {
            if package_vars.len() > 1 {
                // At most one version of each package can be selected
                for i in 0..package_vars.len() {
                    for j in i + 1..package_vars.len() {
                        self.clauses.push(SatClause {
                            literals: vec![
                                SatLiteral { variable_id: package_vars[i].id, negated: true },
                                SatLiteral { variable_id: package_vars[j].id, negated: true },
                            ],
                            description: format!("At most one version of {} can be selected", package_name),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    fn solve(&mut self) -> Result<HashMap<usize, bool>> {
        debug!("Solving SAT problem with {} variables and {} clauses",
            self.count_variables(), self.clauses.len());

        // Use DPLL algorithm for SAT solving
        let mut assignment = HashMap::new();
        let all_variables: Vec<usize> = self.variables.values()
            .flat_map(|vars| vars.iter().map(|v| v.id))
            .collect();

        // Initialize all variables to false
        for var_id in &all_variables {
            assignment.insert(*var_id, false);
        }

        // For simple cases, try to select the latest compatible version of each package
        for (package_name, package_vars) in &self.variables {
            // Find satisfying versions from clauses
            let mut can_select = false;
            for var in package_vars.iter().rev() {  // Try from latest version
                let mut satisfied = true;

                // Check if this version satisfies all relevant clauses
                for clause in &self.clauses {
                    if clause.description.contains(package_name) && clause.description.contains("constraint") {
                        let has_this_var = clause.literals.iter()
                            .any(|lit| lit.variable_id == var.id && !lit.negated);

                        if has_this_var || clause.literals.iter().any(|lit| {
                            assignment.get(&lit.variable_id) == Some(&true) && !lit.negated
                        }) {
                            // This clause is satisfied
                            continue;
                        }

                        // Check if any literal in clause is satisfied
                        let clause_satisfied = clause.literals.iter().any(|lit| {
                            if lit.variable_id == var.id {
                                !lit.negated  // This var would satisfy if selected
                            } else {
                                false
                            }
                        });

                        if !clause_satisfied {
                            satisfied = false;
                            break;
                        }
                    }
                }

                if satisfied {
                    assignment.insert(var.id, true);
                    can_select = true;
                    debug!("Selected {}@{}", package_name, var.version);
                    break;
                }
            }

            // If we need this package but can't select any version, that's an error
            let is_required = self.clauses.iter().any(|c|
                c.description.contains(package_name) && c.description.contains("constraint"));

            if is_required && !can_select {
                // Try DPLL as fallback
                assignment.clear();
                if self.dpll(&mut assignment, &all_variables) {
                    return Ok(assignment);
                } else {
                    return Err(anyhow!("No satisfying assignment found - dependency conflict"));
                }
            }
        }

        // Verify the assignment satisfies all clauses
        if self.all_clauses_satisfied(&assignment) {
            Ok(assignment)
        } else {
            // Fall back to DPLL
            assignment.clear();
            if self.dpll(&mut assignment, &all_variables) {
                Ok(assignment)
            } else {
                Err(anyhow!("No satisfying assignment found - dependency conflict"))
            }
        }
    }

    fn dpll(&self, assignment: &mut HashMap<usize, bool>, unassigned_vars: &[usize]) -> bool {
        // Unit propagation
        loop {
            let mut propagated = false;

            for clause in &self.clauses {
                if let Some(unit_literal) = self.find_unit_literal(clause, assignment) {
                    assignment.insert(unit_literal.variable_id, !unit_literal.negated);
                    propagated = true;
                }
            }

            if !propagated {
                break;
            }
        }

        // Check if all clauses are satisfied
        if self.all_clauses_satisfied(assignment) {
            return true;
        }

        // Check if any clause is unsatisfied
        if self.has_unsatisfied_clause(assignment) {
            return false;
        }

        // Choose next variable to branch on
        if let Some(var) = unassigned_vars.iter()
            .find(|&&v| !assignment.contains_key(&v)) {

            // Try true
            assignment.insert(*var, true);
            if self.dpll(assignment, unassigned_vars) {
                return true;
            }

            // Try false
            assignment.insert(*var, false);
            if self.dpll(assignment, unassigned_vars) {
                return true;
            }

            // Backtrack
            assignment.remove(var);
        }

        false
    }

    fn find_unit_literal<'a>(&self, clause: &'a SatClause, assignment: &HashMap<usize, bool>) -> Option<&'a SatLiteral> {
        let mut unassigned_literal = None;
        let mut unassigned_count = 0;

        for literal in &clause.literals {
            if let Some(&value) = assignment.get(&literal.variable_id) {
                // If literal is satisfied, clause is satisfied
                if (value && !literal.negated) || (!value && literal.negated) {
                    return None;
                }
            } else {
                unassigned_literal = Some(literal);
                unassigned_count += 1;

                if unassigned_count > 1 {
                    return None;
                }
            }
        }

        if unassigned_count == 1 {
            unassigned_literal
        } else {
            None
        }
    }

    fn all_clauses_satisfied(&self, assignment: &HashMap<usize, bool>) -> bool {
        self.clauses.iter().all(|clause| self.is_clause_satisfied(clause, assignment))
    }

    fn has_unsatisfied_clause(&self, assignment: &HashMap<usize, bool>) -> bool {
        self.clauses.iter().any(|clause| self.is_clause_unsatisfied(clause, assignment))
    }

    fn is_clause_satisfied(&self, clause: &SatClause, assignment: &HashMap<usize, bool>) -> bool {
        clause.literals.iter().any(|literal| {
            if let Some(&value) = assignment.get(&literal.variable_id) {
                (value && !literal.negated) || (!value && literal.negated)
            } else {
                false
            }
        })
    }

    fn is_clause_unsatisfied(&self, clause: &SatClause, assignment: &HashMap<usize, bool>) -> bool {
        clause.literals.iter().all(|literal| {
            if let Some(&value) = assignment.get(&literal.variable_id) {
                (value && literal.negated) || (!value && !literal.negated)
            } else {
                false
            }
        })
    }

    fn extract_resolution_result(
        &self,
        solution: HashMap<usize, bool>,
        available_versions: &HashMap<String, Vec<PackageVersion>>,
    ) -> Result<ResolutionResult> {
        let mut resolved_packages = Vec::new();
        let mut conflicts = Vec::new();
        let mut warnings = Vec::new();

        for (package_name, package_vars) in &self.variables {
            let selected_vars: Vec<&SatVariable> = package_vars.iter()
                .filter(|var| solution.get(&var.id) == Some(&true))
                .collect();

            match selected_vars.len() {
                0 => {
                    // Package not selected - this is fine for optional dependencies
                    debug!("Package {} not selected", package_name);
                }
                1 => {
                    let selected_var = selected_vars[0];
                    if let Some(versions) = available_versions.get(package_name) {
                        if let Some(version_info) = versions.iter()
                            .find(|v| v.version == selected_var.version) {

                            resolved_packages.push(ResolvedPackage {
                                name: package_name.clone(),
                                version: selected_var.version.clone(),
                                dependencies: version_info.dependencies.clone(),
                                source_constraint: "resolved".to_string(),
                            });
                        }
                    }
                }
                _ => {
                    // Multiple versions selected - this should not happen with correct constraints
                    conflicts.push(ResolutionConflict {
                        package: package_name.clone(),
                        conflicting_constraints: selected_vars.iter()
                            .map(|v| v.version.clone())
                            .collect(),
                        reason: "Multiple versions selected".to_string(),
                    });
                }
            }
        }

        info!("Resolution complete: {} packages resolved, {} conflicts",
            resolved_packages.len(), conflicts.len());

        Ok(ResolutionResult {
            resolved_packages,
            conflicts,
            warnings,
        })
    }

    fn count_variables(&self) -> usize {
        self.variables.values().map(|vars| vars.len()).sum()
    }
}

impl Default for SatSolver {
    fn default() -> Self {
        Self::new()
    }
}