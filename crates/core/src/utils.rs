use rand::Rng;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use butterflow_models::step::StepAction;
use serde_yaml;

use butterflow_models::{Error, Node, Result, Workflow};

/// Parse a workflow definition from a file
pub fn parse_workflow_file<P: AsRef<Path>>(path: P) -> Result<Workflow> {
    let content = fs::read_to_string(path.as_ref())?;

    // Try to parse as YAML first
    match serde_yaml::from_str::<Workflow>(&content) {
        Ok(workflow) => Ok(workflow),
        Err(yaml_err) => {
            // If YAML parsing fails, try JSON
            match serde_json::from_str::<Workflow>(&content) {
                Ok(workflow) => Ok(workflow),
                Err(json_err) => {
                    // Both parsing attempts failed
                    Err(Error::WorkflowValidation(format!(
                        "Failed to parse workflow file. YAML error: {yaml_err}, JSON error: {json_err}"
                    )))
                }
            }
        }
    }
}

/// Validate a workflow definition
pub fn validate_workflow(workflow: &Workflow, package_path: &Path) -> Result<()> {
    // Check that all node IDs are unique
    let mut node_ids = HashSet::new();
    for node in &workflow.nodes {
        if !node_ids.insert(&node.id) {
            return Err(Error::WorkflowValidation(format!(
                "Duplicate node ID: {}",
                node.id
            )));
        }
    }

    // Check that all template IDs are unique
    let mut template_ids = HashSet::new();
    for template in &workflow.templates {
        if !template_ids.insert(&template.id) {
            return Err(Error::WorkflowValidation(format!(
                "Duplicate template ID: {}",
                template.id
            )));
        }
    }

    // Check that all dependencies exist
    for node in &workflow.nodes {
        for dep_id in &node.depends_on {
            if !node_ids.contains(dep_id) {
                return Err(Error::WorkflowValidation(format!(
                    "Node {} depends on non-existent node: {}",
                    node.id, dep_id
                )));
            }
        }
    }

    // Check for cyclic dependencies
    detect_cycles(&workflow.nodes)?;

    // Check that all template references are valid
    for node in &workflow.nodes {
        for step in &node.steps {
            if let StepAction::UseTemplate(template_use) = &step.action {
                if !template_ids.contains(&template_use.template) {
                    return Err(Error::WorkflowValidation(format!(
                        "Step {} in node {} uses non-existent template: {}",
                        step.name, node.id, template_use.template
                    )));
                }
            } else if let StepAction::JSAstGrep(js_step) = &step.action {
                let js_file_path = package_path.join(&js_step.js_file);
                if !js_file_path.exists() {
                    return Err(Error::WorkflowValidation(format!(
                        "JS file referenced in workflow not found: {}",
                        js_file_path.display()
                    )));
                }
            } else if let StepAction::AstGrep(ast_step) = &step.action {
                let ast_file_path = package_path.join(&ast_step.config_file);
                if !ast_file_path.exists() {
                    return Err(Error::WorkflowValidation(format!(
                        "AST file referenced in workflow not found: {}",
                        ast_file_path.display()
                    )));
                }
            }
        }
    }

    // Check matrix strategies
    for node in &workflow.nodes {
        if let Some(strategy) = &node.strategy {
            if strategy.values.is_none() && strategy.from_state.is_none() {
                return Err(Error::WorkflowValidation(format!(
                    "Matrix strategy for node {} requires either 'values' or 'from_state'",
                    node.id
                )));
            }
        }
    }

    Ok(())
}

/// Detect cycles in the dependency graph
fn detect_cycles(nodes: &[Node]) -> Result<()> {
    // Build adjacency list
    let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in nodes {
        graph.insert(
            &node.id,
            node.depends_on.iter().map(|s| s.as_str()).collect(),
        );
    }

    // Track visited and in-progress nodes
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    // DFS to detect cycles
    for node in nodes {
        if !visited.contains(node.id.as_str()) {
            if let Some(cycle) =
                dfs_cycle_detect(&graph, node.id.as_str(), &mut visited, &mut in_progress)
            {
                return Err(Error::CyclicDependency(cycle));
            }
        }
    }

    Ok(())
}

/// DFS helper for cycle detection
fn dfs_cycle_detect<'a>(
    graph: &HashMap<&'a str, Vec<&'a str>>,
    node: &'a str,
    visited: &mut HashSet<&'a str>,
    in_progress: &mut HashSet<&'a str>,
) -> Option<String> {
    // Mark node as in-progress
    in_progress.insert(node);

    // Visit all neighbors
    if let Some(neighbors) = graph.get(node) {
        for &neighbor in neighbors {
            if in_progress.contains(neighbor) {
                // Found a cycle
                let mut cycle = format!("{node} → {neighbor}");
                let mut current = neighbor;
                while current != node {
                    for &n in graph.keys() {
                        if let Some(deps) = graph.get(n) {
                            if deps.contains(&current) {
                                cycle = format!("{n} → {cycle}");
                                current = n;
                                break;
                            }
                        }
                    }
                }
                return Some(cycle);
            }

            if !visited.contains(neighbor) {
                if let Some(cycle) = dfs_cycle_detect(graph, neighbor, visited, in_progress) {
                    return Some(cycle);
                }
            }
        }
    }

    // Mark node as visited and remove from in-progress
    visited.insert(node);
    in_progress.remove(node);

    None
}

/// Parse parameters from command line arguments
pub fn parse_params(params: &[String]) -> Result<HashMap<String, serde_json::Value>> {
    let mut result = HashMap::new();

    for param in params {
        let parts: Vec<&str> = param.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(Error::Other(format!(
                "Invalid parameter format: {param}. Expected format: key=value"
            )));
        }

        let value = serde_json::from_str(parts[1])
            .unwrap_or_else(|_| serde_json::Value::String(parts[1].to_string()));

        result.insert(parts[0].to_string(), value);
    }

    Ok(result)
}

/// Get environment variables as a HashMap
pub fn get_env_vars() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// Format a duration in seconds as HH:MM:SS
pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;

    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

pub fn get_cache_dir() -> Result<PathBuf> {
    let home_dir = dirs::data_dir()
        .ok_or_else(|| Error::Other("Could not find home directory".to_string()))?;
    let cache_dir = home_dir.join("codemod").join("cache").join("packages");
    Ok(cache_dir)
}

pub fn generate_execution_id() -> String {
    let execution_id: [u8; 20] = rand::rng().random();
    base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        execution_id,
    )
}
