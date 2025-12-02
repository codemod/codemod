use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use butterflow_models::variable::resolve_state_path;
use log::{debug, warn};
#[cfg(feature = "wasm")]
use serde::Serialize;
#[cfg(feature = "wasm")]
use serde_wasm_bindgen::{from_value, to_value};
use uuid::Uuid;
#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

use butterflow_models::node::NodeType;
use butterflow_models::trigger::TriggerType;
use butterflow_models::{Error, Result, Strategy, StrategyType, Task, TaskStatus, WorkflowRun};

#[cfg(feature = "wasm")]
#[wasm_bindgen(typescript_custom_section)]
const MATRIX_TASK_CHANGES: &'static str = r#"
type Uuid = string;

type Task = import("../types").Task;
type WorkflowRun = import("../types").WorkflowRun;
type State = Record<string, unknown>;

interface MatrixTaskChanges {
    new_tasks: Task[];
    tasks_to_mark_wont_do: Uuid[];
    tasks_to_reset_to_pending: Uuid[];
    master_tasks_to_update: Uuid[];
}

interface RunnableTaskChanges {
    tasks_to_await_trigger: Uuid[];
    runnable_tasks: Uuid[];
}
"#;

/// Struct to hold the result of matrix task recompilation calculations
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MatrixTaskChanges {
    /// Tasks that should be created
    pub new_tasks: Vec<Task>,
    /// Tasks that should be marked as WontDo
    pub tasks_to_mark_wont_do: Vec<Uuid>,
    /// Tasks that should be reset to Pending
    pub tasks_to_reset_to_pending: Vec<Uuid>,
    /// Master tasks that should be updated
    pub master_tasks_to_update: Vec<Uuid>,
}

/// Struct to hold the result of finding runnable tasks
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RunnableTaskChanges {
    pub tasks_to_await_trigger: Vec<Uuid>,
    pub runnable_tasks: Vec<Uuid>,
}

#[cfg(not(feature = "wasm"))]
pub struct Scheduler {}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
pub struct Scheduler {}

#[cfg(not(feature = "wasm"))]
impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(feature = "wasm"))]
impl Scheduler {
    pub fn new() -> Self {
        Self {}
    }

    /// Calculate initial tasks for all nodes in a workflow
    pub async fn calculate_initial_tasks(&self, workflow_run: &WorkflowRun) -> Result<Vec<Task>> {
        self.calculate_initial_tasks_internal(workflow_run).await
    }

    /// Calculate changes needed for matrix tasks based on current state
    pub async fn calculate_matrix_task_changes(
        &self,
        workflow_run_id: Uuid,
        workflow_run: &WorkflowRun,
        tasks: &[Task],
        state: &HashMap<String, serde_json::Value>,
    ) -> Result<MatrixTaskChanges> {
        self.calculate_matrix_task_changes_internal(workflow_run_id, workflow_run, tasks, state)
            .await
    }

    /// Find tasks that can be executed
    pub async fn find_runnable_tasks(
        &self,
        workflow_run: &WorkflowRun,
        tasks: &[Task],
    ) -> Result<RunnableTaskChanges> {
        self.find_runnable_tasks_internal(workflow_run, tasks).await
    }
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl Scheduler {
    // Expose constructor to WASM
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {}
    }

    // --- WASM Exposed Methods ---

    /// Calculate initial tasks for a workflow run (WASM API).
    #[wasm_bindgen(js_name = calculateInitialTasks, unchecked_return_type = "Task[]")]
    pub async fn calculate_initial_tasks_wasm(
        &self,
        #[wasm_bindgen(unchecked_param_type = "WorkflowRun")] workflow_run_js: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let workflow_run: WorkflowRun = from_value(workflow_run_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize WorkflowRun: {}", e)))?;
        let serializer = serde_wasm_bindgen::Serializer::json_compatible()
            .serialize_maps_as_objects(true)
            .serialize_missing_as_null(true);

        let result = self.calculate_initial_tasks_internal(&workflow_run).await;

        match result {
            Ok(tasks) => tasks
                .serialize(&serializer)
                .map_err(|e| JsValue::from_str(&format!("Failed to serialize tasks: {}", e))),
            Err(e) => Err(JsValue::from_str(&e.to_string())),
        }
    }

    /// Calculate changes needed for matrix tasks based on current state (WASM API).
    #[wasm_bindgen(js_name = calculateMatrixTaskChanges, unchecked_return_type = "MatrixTaskChanges")]
    pub async fn calculate_matrix_task_changes_wasm(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Uuid")] workflow_run_id_js: JsValue, // Expect Uuid as string
        #[wasm_bindgen(unchecked_param_type = "WorkflowRun")] workflow_run_js: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Task[]")] tasks_js: JsValue,
        #[wasm_bindgen(unchecked_param_type = "State")] state_js: JsValue, // Expect JSON object
    ) -> std::result::Result<JsValue, JsValue> {
        let workflow_run_id_str: String = from_value(workflow_run_id_js).map_err(|e| {
            JsValue::from_str(&format!("Failed to deserialize workflow_run_id: {}", e))
        })?;
        let workflow_run_id = Uuid::parse_str(&workflow_run_id_str).map_err(|e| {
            JsValue::from_str(&format!("Invalid UUID format for workflow_run_id: {}", e))
        })?;

        let workflow_run: WorkflowRun = from_value(workflow_run_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize WorkflowRun: {}", e)))?;
        let tasks: Vec<Task> = from_value(tasks_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize tasks: {}", e)))?;
        let state: HashMap<String, serde_json::Value> = from_value(state_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize state: {}", e)))?;

        let result = self
            .calculate_matrix_task_changes_internal(workflow_run_id, &workflow_run, &tasks, &state)
            .await;

        match result {
            Ok(changes) => to_value(&changes).map_err(|e| {
                JsValue::from_str(&format!("Failed to serialize MatrixTaskChanges: {}", e))
            }),
            Err(e) => Err(JsValue::from_str(&e.to_string())),
        }
    }

    /// Find tasks that can be executed (WASM API).
    #[wasm_bindgen(js_name = findRunnableTasks, unchecked_return_type = "RunnableTaskChanges")]
    pub async fn find_runnable_tasks_wasm(
        &self,
        #[wasm_bindgen(unchecked_param_type = "WorkflowRun")] workflow_run_js: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Task[]")] tasks_js: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let workflow_run: WorkflowRun = from_value(workflow_run_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize WorkflowRun: {}", e)))?;
        let tasks: Vec<Task> = from_value(tasks_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize tasks: {}", e)))?;

        let result = self
            .find_runnable_tasks_internal(&workflow_run, &tasks)
            .await;

        match result {
            Ok(changes) => to_value(&changes).map_err(|e| {
                JsValue::from_str(&format!("Failed to serialize RunnableTaskChanges: {}", e))
            }),
            Err(e) => Err(JsValue::from_str(&e.to_string())),
        }
    }
}

/// Create a stable hash from matrix values for consistent task identification
/// This ensures that the same logical matrix values always produce the same hash,
/// regardless of JSON serialization order or other inconsistencies.
fn create_stable_hash(item: &serde_json::Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_value_stable(item, &mut hasher);
    hasher.finish()
}

fn hash_value_stable<H: Hasher>(value: &serde_json::Value, hasher: &mut H) {
    match value {
        serde_json::Value::Null => {
            0u8.hash(hasher);
        }
        serde_json::Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(hasher);
            if let Some(i) = n.as_i64() {
                0u8.hash(hasher); // integer marker
                i.hash(hasher);
            } else if let Some(u) = n.as_u64() {
                1u8.hash(hasher); // unsigned marker
                u.hash(hasher);
            } else if let Some(f) = n.as_f64() {
                2u8.hash(hasher); // float marker
                f.to_bits().hash(hasher);
            }
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Array(arr) => {
            4u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_value_stable(item, hasher);
            }
        }
        serde_json::Value::Object(obj) => {
            5u8.hash(hasher);
            obj.len().hash(hasher);

            let mut sorted_keys: Vec<_> = obj.keys().collect();
            sorted_keys.sort();

            for key in sorted_keys {
                key.hash(hasher);
                hash_value_stable(&obj[key], hasher);
            }
        }
    }
}

/// Helper function to create hash from HashMap matrix values
/// Excludes keys starting with "_meta_" from hash calculation as these are considered
/// metadata fields that don't affect task execution logic
fn create_matrix_hash(matrix_values: &HashMap<String, serde_json::Value>) -> u64 {
    // Filter out metadata keys that shouldn't affect task identity
    let filtered_values: HashMap<String, serde_json::Value> = matrix_values
        .iter()
        .filter(|(key, _)| !key.starts_with("_meta_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let json_value = serde_json::to_value(&filtered_values).unwrap_or(serde_json::Value::Null);
    create_stable_hash(&json_value)
}

// Internal implementation shared by both Rust and WASM APIs
impl Scheduler {
    async fn calculate_initial_tasks_internal(
        &self,
        workflow_run: &WorkflowRun,
    ) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();

        for node in &workflow_run.workflow.nodes {
            // Check if the node has a matrix strategy
            if let Some(Strategy {
                r#type: StrategyType::Matrix,
                values,
                from_state: _,
            }) = &node.strategy
            {
                // Create a master task for the matrix
                let master_task = Task::new(workflow_run.id, node.id.clone(), true);
                tasks.push(master_task.clone());

                // If the matrix uses values, create tasks for each value
                if let Some(values) = values {
                    for value in values {
                        // Create a task for each matrix value
                        let task = Task::new_matrix(
                            workflow_run.id,
                            node.id.clone(),
                            master_task.id,
                            value.clone(),
                        );
                        tasks.push(task);
                    }
                }
                // If the matrix uses state, tasks will be created during recompilation
            } else {
                // Create a single task for the node
                let task = Task::new(workflow_run.id, node.id.clone(), false);
                tasks.push(task);
            }
        }

        Ok(tasks)
    }

    /// Calculate changes needed for matrix tasks based on current state
    async fn calculate_matrix_task_changes_internal(
        &self,
        workflow_run_id: Uuid,
        workflow_run: &WorkflowRun,
        tasks: &[Task],
        state: &HashMap<String, serde_json::Value>,
    ) -> Result<MatrixTaskChanges> {
        let mut new_tasks = Vec::new();
        let mut tasks_to_mark_wont_do = Vec::new();
        let mut tasks_to_reset_to_pending = Vec::new();
        let mut master_tasks_to_update = Vec::new();

        for node in &workflow_run.workflow.nodes {
            if let Some(Strategy {
                r#type: StrategyType::Matrix,
                from_state: Some(state_key), // Only process matrix nodes using from_state
                .. // Use .. to ignore other fields like `values`
            }) = &node.strategy
            {
                debug!(
                    "Calculating changes for matrix node '{}' using state key '{}'",
                    node.id, state_key
                );

                // Find the master task for this node
                let master_task_id =
                    match tasks.iter().find(|t| t.node_id == node.id && t.is_master) {
                        Some(master) => master.id,
                        None => {
                            // Master task doesn't exist yet, create it
                            let new_master_task = Task::new(workflow_run_id, node.id.clone(), true);
                            new_tasks.push(new_master_task.clone());
                            master_tasks_to_update.push(new_master_task.id);
                            new_master_task.id
                        }
                    };

                // Add master task to update list if not already there
                if !master_tasks_to_update.contains(&master_task_id) {
                    master_tasks_to_update.push(master_task_id);
                }

                // Get the current value from the state
                let state_value = resolve_state_path(state, state_key);

                // --- Calculate Values for Current State Items ---
                let mut current_item_values = Vec::new();

                match state_value {
                    Ok(serde_json::Value::Array(items)) => {
                        for item in items {
                            current_item_values.push(item.clone());
                        }
                        debug!("Found {} items in state array '{}'", items.len(), state_key);
                    }
                    Ok(serde_json::Value::Object(_obj)) => {
                        // Object mapping not fully supported yet
                        warn!("Matrix from_state for object key '{state_key}' is not yet fully supported, skipping.");
                        continue;
                    }
                    Ok(_) => {
                        // State key not found or not an array/object
                        debug!("State key '{}' for matrix node '{}' is missing or not an array/object.", state_key, node.id);
                    }
                    Err(_) => {
                        debug!("Could not resolve state path '{state_key}'");
                    }
                }

                let existing_tasks_for_node = tasks.iter().filter(|t| {
                    t.master_task_id == Some(master_task_id) && t.matrix_values.is_some()
                });

                // --- Compare with Existing Tasks ---
                let existing_child_tasks_by_hash: HashMap<u64, &Task> = existing_tasks_for_node
                    .map(|t| {
                        let hash = create_matrix_hash(t.matrix_values.as_ref().unwrap());
                        (hash, t)
                    })
                    .collect();

                let existing_child_hashes: HashSet<u64> =
                    existing_child_tasks_by_hash.keys().cloned().collect();

                debug!(
                    "Found {} existing child tasks for node '{}'",
                    existing_child_tasks_by_hash.len(),
                    node.id
                );

                // --- Identify Tasks to Create ---
                let mut current_item_hashes = HashSet::new();

                for item_value in &current_item_values {
                    // Convert state item to matrix_data format first, then hash that
                    // This ensures we're comparing the same representation
                    let matrix_data = match item_value.as_object() {
                        Some(obj) => obj
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<HashMap<_, _>>(),
                        None => {
                            warn!(
                                "Matrix item for node '{}' is not a JSON object, skipping: {:?}",
                                node.id,
                                item_value
                            );
                            continue; // Skip this item
                        }
                    };

                    let item_hash = create_matrix_hash(&matrix_data);
                    current_item_hashes.insert(item_hash);

                    if !existing_child_hashes.contains(&item_hash) {
                        let new_task = Task::new_matrix(
                            workflow_run_id,
                            node.id.clone(),
                            master_task_id,
                            matrix_data,
                        );
                        debug!(
                            "Need to create new task for node '{}', hash: {}, value: {:?}",
                            node.id, item_hash, item_value
                        );
                        new_tasks.push(new_task);
                    }
                }

                // --- Identify Tasks to Reset to Pending ---
                // When state changes (this function is called), existing tasks that are
                // still valid (hash matches) but are in Failed state should be reset to
                // Pending.
                for (task_hash, task) in &existing_child_tasks_by_hash {
                    if current_item_hashes.contains(task_hash) {
                        // Task hash still matches current state - task is still valid
                        if task.status == TaskStatus::Failed {
                            debug!(
                                "Need to reset task {} (hash: {}, matrix_values: {:?}) for node '{}' from Failed to Pending",
                                task.id, task_hash, task.matrix_values, node.id
                            );
                            tasks_to_reset_to_pending.push(task.id);
                        }
                    }
                }

                // --- Identify Tasks to Mark as WontDo ---
                for (task_hash, task) in &existing_child_tasks_by_hash {
                    if !current_item_hashes.contains(task_hash) {
                        // Mark as WontDo only if it's not already in a terminal state
                        if !matches!(
                            task.status,
                            TaskStatus::Completed | TaskStatus::WontDo
                        ) {
                            debug!(
                                "Need to mark task {} (hash: {}, matrix_values: {:?}) for node '{}' as WontDo",
                                task.id, task_hash, task.matrix_values, node.id
                            );
                            tasks_to_mark_wont_do.push(task.id);
                        }
                    }
                }
            }
        }

        Ok(MatrixTaskChanges {
            new_tasks,
            tasks_to_mark_wont_do,
            tasks_to_reset_to_pending,
            master_tasks_to_update,
        })
    }

    /// Find tasks that can be executed
    async fn find_runnable_tasks_internal(
        &self,
        workflow_run: &WorkflowRun,
        tasks: &[Task],
    ) -> Result<RunnableTaskChanges> {
        let mut runnable_tasks = Vec::new();
        let mut tasks_to_await_trigger = Vec::new();

        for task in tasks {
            // Only consider pending tasks and non-master tasks
            if task.status != TaskStatus::Pending || task.is_master {
                continue;
            }

            // Get the node for this task
            let node = workflow_run
                .workflow
                .nodes
                .iter()
                .find(|n| n.id == task.node_id)
                .ok_or_else(|| Error::NodeNotFound(task.node_id.clone()))?;

            // Check if the node has a manual trigger
            if node.r#type == NodeType::Manual
                || node
                    .trigger
                    .as_ref()
                    .map(|t| t.r#type == TriggerType::Manual)
                    .unwrap_or(false)
            {
                tasks_to_await_trigger.push(task.id);
                continue;
            }

            // Check if all dependencies are satisfied
            let mut dependencies_satisfied = true;
            for dep_id in &node.depends_on {
                // Find all tasks for this dependency
                let dep_tasks: Vec<&Task> = tasks.iter().filter(|t| t.node_id == *dep_id).collect();

                // If there are no tasks for this dependency, it's not satisfied
                if dep_tasks.is_empty() {
                    dependencies_satisfied = false;
                    break;
                }

                // Check if all tasks for this dependency are completed
                let all_completed = dep_tasks.iter().all(|t| t.status == TaskStatus::Completed);

                if !all_completed {
                    dependencies_satisfied = false;
                    break;
                }
            }

            if dependencies_satisfied {
                runnable_tasks.push(task.id);
            }
        }

        Ok(RunnableTaskChanges {
            tasks_to_await_trigger,
            runnable_tasks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_stable_hash_consistency() {
        let value1 = json!({
            "name": "John",
            "age": 30,
            "active": true
        });

        let value2 = json!({
            "age": 30,
            "name": "John",
            "active": true
        });

        // Should have same hash despite different key order
        assert_eq!(create_stable_hash(&value1), create_stable_hash(&value2));
    }

    #[test]
    fn test_different_values_different_hashes() {
        let value1 = json!({"name": "John", "age": 30});
        let value2 = json!({"name": "Jane", "age": 30});

        assert_ne!(create_stable_hash(&value1), create_stable_hash(&value2));
    }

    #[test]
    fn test_nested_objects() {
        let value1 = json!({
            "user": {
                "name": "John",
                "details": {
                    "age": 30,
                    "city": "NYC"
                }
            }
        });

        let value2 = json!({
            "user": {
                "details": {
                    "city": "NYC",
                    "age": 30
                },
                "name": "John"
            }
        });

        assert_eq!(create_stable_hash(&value1), create_stable_hash(&value2));
    }

    #[test]
    fn test_matrix_hash_consistency() {
        // Test that state items and their converted matrix_data have the same hash
        let state_item = json!({
            "team": "unassigned",
            "shard": "1/6",
            "shardId": "unassigned 1/6",
            "files": ["file1.ts", "file2.ts"],
            "count": 42
        });

        // Simulate the conversion that happens in matrix task creation
        let matrix_data = state_item
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<std::collections::HashMap<_, _>>();

        let state_hash = create_matrix_hash(&matrix_data);
        let matrix_hash = create_matrix_hash(&matrix_data);

        // Should be equal since we're using the same data
        let converted_matrix_data = state_item
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        let converted_hash = create_matrix_hash(&converted_matrix_data);

        assert_eq!(
            matrix_hash, converted_hash,
            "Matrix hashes should be equal when using same conversion"
        );

        assert_eq!(
            state_hash, converted_hash,
            "State and converted matrix hashes should be equal"
        );
    }

    #[test]
    fn test_meta_key_exclusion() {
        // Test that keys starting with "_meta_" are excluded from hash calculation
        let matrix_data_1 = [
            (
                "team".to_string(),
                serde_json::Value::String("frontend".to_string()),
            ),
            (
                "shard".to_string(),
                serde_json::Value::String("1/3".to_string()),
            ),
            (
                "_meta_timestamp".to_string(),
                serde_json::Value::String("2024-01-01T00:00:00Z".to_string()),
            ),
            (
                "_meta_build_id".to_string(),
                serde_json::Value::Number(serde_json::Number::from(12345)),
            ),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        let matrix_data_2 = [
            (
                "team".to_string(),
                serde_json::Value::String("frontend".to_string()),
            ),
            (
                "shard".to_string(),
                serde_json::Value::String("1/3".to_string()),
            ),
            (
                "_meta_timestamp".to_string(),
                serde_json::Value::String("2024-01-01T12:00:00Z".to_string()),
            ), // Different timestamp
            (
                "_meta_build_id".to_string(),
                serde_json::Value::Number(serde_json::Number::from(67890)),
            ), // Different build ID
            (
                "_meta_extra_field".to_string(),
                serde_json::Value::String("extra".to_string()),
            ), // Additional meta field
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        let hash_1 = create_matrix_hash(&matrix_data_1);
        let hash_2 = create_matrix_hash(&matrix_data_2);

        assert_eq!(
            hash_1, hash_2,
            "Matrix hashes should be equal even when _meta_ fields differ"
        );

        // Test that non-meta fields still affect the hash
        let matrix_data_3 = [
            (
                "team".to_string(),
                serde_json::Value::String("backend".to_string()),
            ), // Different team
            (
                "shard".to_string(),
                serde_json::Value::String("1/3".to_string()),
            ),
            (
                "_meta_timestamp".to_string(),
                serde_json::Value::String("2024-01-01T00:00:00Z".to_string()),
            ),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        let hash_3 = create_matrix_hash(&matrix_data_3);

        assert_ne!(
            hash_1, hash_3,
            "Matrix hashes should differ when non-meta fields differ"
        );
    }
}
