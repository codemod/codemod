#![cfg(test)]

use std::collections::HashMap;

use butterflow_scheduler::Scheduler;
use serde_json::json;
use uuid::Uuid;

use butterflow_models::node::{Node, NodeType};
use butterflow_models::runtime::{Runtime, RuntimeType};
use butterflow_models::step::{Step, StepAction};
use butterflow_models::strategy::{Strategy, StrategyType};
use butterflow_models::task::{Task, TaskStatus};
use butterflow_models::trigger::{Trigger, TriggerType};
use butterflow_models::workflow::{Workflow, WorkflowRun};

// --- Helper Functions ---

fn create_basic_node(id: &str, depends_on: Vec<&str>) -> Node {
    Node {
        id: id.to_string(),
        name: format!("Node {id}"),
        description: None,
        r#type: NodeType::Automatic,
        depends_on: depends_on.into_iter().map(String::from).collect(),
        trigger: None,
        strategy: None,
        runtime: Some(Runtime {
            r#type: RuntimeType::Direct,
            image: None,
            working_dir: None,
            user: None,
            network: None,
            options: None,
        }),
        steps: vec![Step {
            id: Some(format!("step1_{id}")),
            name: "Step 1".to_string(),
            action: StepAction::RunScript(format!("echo 'Running {id}'")),
            env: None,
            condition: None,
        }],
        env: HashMap::new(),
    }
}

fn create_matrix_node_values(
    id: &str,
    depends_on: Vec<&str>,
    values: Vec<HashMap<String, serde_json::Value>>,
) -> Node {
    Node {
        id: id.to_string(),
        name: format!("Node {id}"),
        description: None,
        r#type: NodeType::Automatic,
        depends_on: depends_on.into_iter().map(String::from).collect(),
        trigger: None,
        strategy: Some(Strategy {
            r#type: StrategyType::Matrix,
            values: Some(values),
            from_state: None,
        }),
        runtime: Some(Runtime {
            r#type: RuntimeType::Direct,
            image: None,
            working_dir: None,
            user: None,
            network: None,
            options: None,
        }),
        steps: vec![Step {
            id: Some(id.to_string()),
            name: "Step 1".to_string(),
            action: StepAction::RunScript(format!("echo 'Running matrix {id}'")),
            env: None,
            condition: None,
        }],
        env: HashMap::new(),
    }
}

fn create_matrix_node_from_state(id: &str, depends_on: Vec<&str>, state_key: &str) -> Node {
    Node {
        id: id.to_string(),
        name: format!("Node {id}"),
        description: None,
        r#type: NodeType::Automatic,
        depends_on: depends_on.into_iter().map(String::from).collect(),
        trigger: None,
        strategy: Some(Strategy {
            r#type: StrategyType::Matrix,
            values: None,
            from_state: Some(state_key.to_string()),
        }),
        runtime: Some(Runtime {
            r#type: RuntimeType::Direct,
            image: None,
            working_dir: None,
            user: None,
            network: None,
            options: None,
        }),
        steps: vec![Step {
            id: Some(id.to_string()),
            name: "Step 1".to_string(),
            action: StepAction::RunScript(format!("echo 'Running matrix from state {id}'")),
            env: None,
            condition: None,
        }],
        env: HashMap::new(),
    }
}

fn create_manual_node(
    id: &str,
    depends_on: Vec<&str>,
    node_type: NodeType,
    trigger_type: Option<TriggerType>,
) -> Node {
    Node {
        id: id.to_string(),
        name: format!("Node {id}"),
        description: None,
        r#type: node_type,
        depends_on: depends_on.into_iter().map(String::from).collect(),
        trigger: trigger_type.map(|t| Trigger { r#type: t }),
        strategy: None,
        runtime: Some(Runtime {
            r#type: RuntimeType::Direct,
            image: None,
            working_dir: None,
            user: None,
            network: None,
            options: None,
        }),
        steps: vec![Step {
            id: Some(id.to_string()),
            name: "Step 1".to_string(),
            action: StepAction::RunScript(format!("echo 'Running manual {id}'")),
            env: None,
            condition: None,
        }],
        env: HashMap::new(),
    }
}

fn create_test_workflow(nodes: Vec<Node>) -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        templates: vec![],
        nodes,
        params: None,
    }
}

fn create_test_run(workflow: Workflow) -> WorkflowRun {
    WorkflowRun {
        id: Uuid::new_v4(),
        workflow,
        status: butterflow_models::WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: vec![],
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        target_path: None,
        capabilities: None,
    }
}

// --- Test Cases ---

#[tokio::test]
async fn test_calculate_initial_tasks_simple() {
    let scheduler = Scheduler::new();
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_basic_node("node2", vec!["node1"]),
    ]);
    let run = create_test_run(workflow);

    let tasks = scheduler.calculate_initial_tasks(&run).await.unwrap();

    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().any(|t| t.node_id == "node1" && !t.is_master));
    assert!(tasks.iter().any(|t| t.node_id == "node2" && !t.is_master));
    assert_eq!(tasks.iter().filter(|t| t.is_master).count(), 0);
}

#[tokio::test]
async fn test_calculate_initial_tasks_matrix_values() {
    let scheduler = Scheduler::new();
    let matrix_values = vec![
        HashMap::from([("k".to_string(), serde_json::Value::String("v1".to_string()))]),
        HashMap::from([("k".to_string(), serde_json::Value::String("v2".to_string()))]),
    ];
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_matrix_node_values("node2", vec!["node1"], matrix_values.clone()),
    ]);
    let run = create_test_run(workflow);

    let tasks = scheduler.calculate_initial_tasks(&run).await.unwrap();

    assert_eq!(tasks.len(), 4); // 1 for node1, 1 master for node2, 2 children for node2
    assert!(tasks.iter().any(|t| t.node_id == "node1" && !t.is_master));
    assert!(tasks.iter().any(|t| t.node_id == "node2" && t.is_master));
    let master_task_id = tasks
        .iter()
        .find(|t| t.node_id == "node2" && t.is_master)
        .unwrap()
        .id;

    let child_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.node_id == "node2" && !t.is_master)
        .collect();
    assert_eq!(child_tasks.len(), 2);
    assert!(child_tasks
        .iter()
        .all(|t| t.master_task_id == Some(master_task_id)));
    assert!(child_tasks
        .iter()
        .any(|t| t.matrix_values == Some(matrix_values[0].clone())));
    assert!(child_tasks
        .iter()
        .any(|t| t.matrix_values == Some(matrix_values[1].clone())));
}

#[tokio::test]
async fn test_calculate_initial_tasks_matrix_from_state() {
    let scheduler = Scheduler::new();
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_matrix_node_from_state("node2", vec!["node1"], "my_state_key"),
    ]);
    let run = create_test_run(workflow);

    let tasks = scheduler.calculate_initial_tasks(&run).await.unwrap();

    // Initially, only the master task for the matrix node should be created
    assert_eq!(tasks.len(), 2); // 1 for node1, 1 master for node2
    assert!(tasks.iter().any(|t| t.node_id == "node1" && !t.is_master));
    assert!(tasks.iter().any(|t| t.node_id == "node2" && t.is_master));
    assert_eq!(
        tasks
            .iter()
            .filter(|t| t.node_id == "node2" && !t.is_master)
            .count(),
        0
    );
}

#[tokio::test]
async fn test_calculate_matrix_task_changes_no_master_yet() {
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_matrix_node_from_state("node2", vec!["node1"], "items"),
    ]);
    let run = create_test_run(workflow.clone());

    // Simulate state having items
    let initial_state = HashMap::from([("items".to_string(), json!([{"id": "a"}, {"id": "b"}]))]);

    let scheduler = Scheduler::new();

    // Simulate existing tasks (only node1's task)
    let initial_tasks = vec![Task::new(run.id, "node1".to_string(), false)];

    let changes = scheduler
        .calculate_matrix_task_changes(run.id, &run, &initial_tasks, &initial_state)
        .await
        .unwrap();

    assert_eq!(changes.new_tasks.len(), 3); // Master task + 2 child tasks
    assert!(changes
        .new_tasks
        .iter()
        .any(|t| t.is_master && t.node_id == "node2"));
    assert_eq!(changes.tasks_to_mark_wont_do.len(), 0);
    assert_eq!(changes.master_tasks_to_update.len(), 1); // The new master task
    assert_eq!(changes.new_tasks.iter().filter(|t| !t.is_master).count(), 2);
    assert!(changes
        .new_tasks
        .iter()
        .filter(|t| !t.is_master)
        .any(|t| t.matrix_values
            == Some(HashMap::from([(
                "id".to_string(),
                serde_json::Value::String("a".to_string())
            )]))));
    assert!(changes
        .new_tasks
        .iter()
        .filter(|t| !t.is_master)
        .any(|t| t.matrix_values
            == Some(HashMap::from([(
                "id".to_string(),
                serde_json::Value::String("b".to_string())
            )]))));
}

#[tokio::test]
async fn test_calculate_matrix_task_changes_add_remove() {
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_matrix_node_from_state("node2", vec!["node1"], "items"),
    ]);
    let run = create_test_run(workflow.clone());
    let node2_id = "node2".to_string();

    // --- Initial Setup ---
    let master_task = Task::new(run.id, node2_id.clone(), true);
    let task_a = Task::new_matrix(
        run.id,
        node2_id.clone(),
        master_task.id,
        HashMap::from([("id".to_string(), serde_json::Value::String("a".to_string()))]),
    );
    let task_b = Task::new_matrix(
        run.id,
        node2_id.clone(),
        master_task.id,
        HashMap::from([("id".to_string(), serde_json::Value::String("b".to_string()))]),
    );

    // Simulate existing tasks
    let initial_tasks = vec![
        Task::new(run.id, "node1".to_string(), false), // Node 1 task
        master_task.clone(),
        task_a.clone(),
        task_b.clone(),
    ];

    // Simulate new state: remove 'b', add 'c'
    let new_state = HashMap::from([(
        "items".to_string(),
        json!([{"id": "a"}, {"id": "c"}]), // 'b' removed, 'c' added
    )]);

    let scheduler = Scheduler::new();

    // --- Calculation ---
    let changes = scheduler
        .calculate_matrix_task_changes(run.id, &run, &initial_tasks, &new_state)
        .await
        .unwrap();

    // --- Assertions ---
    // Should create task 'c'
    assert_eq!(changes.new_tasks.len(), 1);
    assert!(
        changes.new_tasks[0].matrix_values
            == Some(HashMap::from([(
                "id".to_string(),
                serde_json::Value::String("c".to_string())
            )]))
    );
    assert_eq!(changes.new_tasks[0].master_task_id, Some(master_task.id));

    // Should mark task 'b' as WontDo
    assert_eq!(changes.tasks_to_mark_wont_do.len(), 1);
    assert_eq!(changes.tasks_to_mark_wont_do[0], task_b.id);

    // Should update the master task
    assert_eq!(changes.master_tasks_to_update.len(), 1);
    assert_eq!(changes.master_tasks_to_update[0], master_task.id);
}

#[tokio::test]
async fn test_calculate_matrix_task_changes_state_key_missing() {
    let workflow = create_test_workflow(vec![create_matrix_node_from_state(
        "node2",
        vec![],
        "items",
    )]);
    let run = create_test_run(workflow.clone());
    let node2_id = "node2".to_string();

    let master_task = Task::new(run.id, node2_id.clone(), true);
    let initial_tasks = vec![master_task.clone()];

    let scheduler = Scheduler::new();

    let changes = scheduler
        .calculate_matrix_task_changes(run.id, &run, &initial_tasks, &HashMap::new())
        .await
        .unwrap();

    assert_eq!(changes.new_tasks.len(), 0);
    assert_eq!(changes.tasks_to_mark_wont_do.len(), 0);
    assert_eq!(changes.master_tasks_to_update.len(), 1); // Still updates the master
    assert_eq!(changes.master_tasks_to_update[0], master_task.id);
}

#[tokio::test]
async fn test_find_runnable_tasks_simple_dependency() {
    let scheduler = Scheduler::new();
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_basic_node("node2", vec!["node1"]),
    ]);
    let run = create_test_run(workflow);

    // Case 1: node1 pending, node2 pending -> only node1 runnable
    let tasks1 = vec![
        Task::new(run.id, "node1".to_string(), false),
        Task::new(run.id, "node2".to_string(), false),
    ];
    let runnable1 = scheduler.find_runnable_tasks(&run, &tasks1).await.unwrap();
    assert_eq!(runnable1.runnable_tasks.len(), 1);
    assert_eq!(runnable1.runnable_tasks[0], tasks1[0].id);

    // Case 2: node1 completed, node2 pending -> only node2 runnable
    let mut tasks2 = tasks1.clone();
    tasks2[0].status = TaskStatus::Completed;
    let runnable2 = scheduler.find_runnable_tasks(&run, &tasks2).await.unwrap();
    assert_eq!(runnable2.runnable_tasks.len(), 1);
    assert_eq!(runnable2.runnable_tasks[0], tasks2[1].id);

    // Case 3: node1 running, node2 pending -> no tasks runnable
    let mut tasks3 = tasks1.clone();
    tasks3[0].status = TaskStatus::Running;
    let runnable3 = scheduler.find_runnable_tasks(&run, &tasks3).await.unwrap();
    assert_eq!(runnable3.runnable_tasks.len(), 0);

    // Case 4: node1 completed, node2 completed -> no tasks runnable
    let mut tasks4 = tasks1.clone();
    tasks4[0].status = TaskStatus::Completed;
    tasks4[1].status = TaskStatus::Completed;
    let runnable4 = scheduler.find_runnable_tasks(&run, &tasks4).await.unwrap();
    assert_eq!(runnable4.runnable_tasks.len(), 0);
}

#[tokio::test]
async fn test_find_runnable_tasks_matrix_dependency() {
    let scheduler = Scheduler::new();
    let matrix_values = vec![
        HashMap::from([("k".to_string(), serde_json::Value::String("v1".to_string()))]),
        HashMap::from([("k".to_string(), serde_json::Value::String("v2".to_string()))]),
    ];
    let workflow = create_test_workflow(vec![
        create_matrix_node_values("node1", vec![], matrix_values.clone()),
        create_basic_node("node2", vec!["node1"]),
    ]);
    let run = create_test_run(workflow);

    let node1_master = Task::new(run.id, "node1".to_string(), true);
    let node1_child1 = Task::new_matrix(
        run.id,
        "node1".to_string(),
        node1_master.id,
        matrix_values[0].clone(),
    );
    let node1_child2 = Task::new_matrix(
        run.id,
        "node1".to_string(),
        node1_master.id,
        matrix_values[1].clone(),
    );
    let node2_task = Task::new(run.id, "node2".to_string(), false);

    // Case 1: All node1 tasks pending -> only node1 children runnable
    let tasks1 = vec![
        node1_master.clone(),
        node1_child1.clone(),
        node1_child2.clone(),
        node2_task.clone(),
    ];
    let runnable1 = scheduler.find_runnable_tasks(&run, &tasks1).await.unwrap();
    assert_eq!(runnable1.runnable_tasks.len(), 2);
    assert!(runnable1.runnable_tasks.contains(&node1_child1.id));
    assert!(runnable1.runnable_tasks.contains(&node1_child2.id));

    // Case 2: One node1 child completed, one pending -> node2 not runnable yet
    let mut tasks2 = tasks1.clone();
    tasks2[1].status = TaskStatus::Completed; // child1 completed
    let runnable2 = scheduler.find_runnable_tasks(&run, &tasks2).await.unwrap();
    assert_eq!(runnable2.runnable_tasks.len(), 1); // Only child2 is runnable
    assert_eq!(runnable2.runnable_tasks[0], node1_child2.id);

    // Case 3: Both node1 children completed -> node2 runnable
    let mut tasks3 = tasks1.clone();
    tasks3[1].status = TaskStatus::Completed; // child1 completed
    tasks3[2].status = TaskStatus::Completed; // child2 completed
                                              // We also need to update the master task status manually for the test scenario
    tasks3[0].status = TaskStatus::Completed;

    let runnable3 = scheduler.find_runnable_tasks(&run, &tasks3).await.unwrap();
    assert_eq!(runnable3.runnable_tasks.len(), 1);
    assert_eq!(runnable3.runnable_tasks[0], node2_task.id);
}

#[tokio::test]
async fn test_find_runnable_tasks_manual_trigger() {
    let scheduler = Scheduler::new();

    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_manual_node(
            "node2",
            vec!["node1"],
            NodeType::Automatic,
            Some(TriggerType::Manual),
        ),
    ]);
    let run = create_test_run(workflow);

    let mut task1 = Task::new(run.id, "node1".to_string(), false);
    let task2 = Task::new(run.id, "node2".to_string(), false);

    task1.status = TaskStatus::Completed;

    let task2_id = task2.id;

    // Now, node2's dependencies are met, but it has a manual trigger.
    let runnable_after_node1 = scheduler
        .find_runnable_tasks(&run, &vec![task1, task2])
        .await
        .unwrap();

    // No tasks should be directly runnable
    assert_eq!(runnable_after_node1.runnable_tasks.len(), 0);

    // Verify that task2's status *in the adapter* is now AwaitingTrigger
    assert_eq!(runnable_after_node1.tasks_to_await_trigger.len(), 1);
    assert_eq!(runnable_after_node1.tasks_to_await_trigger[0], task2_id);
}

#[tokio::test]
async fn test_find_runnable_tasks_manual_node_type() {
    let scheduler = Scheduler::new();

    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_manual_node("node2", vec!["node1"], NodeType::Manual, None), // Manual Node Type
    ]);
    let run = create_test_run(workflow);

    let mut task1 = Task::new(run.id, "node1".to_string(), false);
    let task2 = Task::new(run.id, "node2".to_string(), false);
    let task2_id = task2.id;

    task1.status = TaskStatus::Completed;

    // Node2's dependencies are met, but it's a manual node type.
    let runnable_after_node1 = scheduler
        .find_runnable_tasks(&run, &vec![task1, task2])
        .await
        .unwrap();

    // No tasks should be directly runnable
    assert_eq!(runnable_after_node1.runnable_tasks.len(), 0);

    // Verify that task2's status *in the adapter* is now AwaitingTrigger
    assert_eq!(runnable_after_node1.tasks_to_await_trigger.len(), 1);
    assert_eq!(runnable_after_node1.tasks_to_await_trigger[0], task2_id);
}

/// Test that failed tasks are reset to pending when state changes but hashes match.
/// This simulates the scenario where a parent node (like evaluate-shards) is re-triggered,
/// produces the same shards, but the failed child tasks should get a chance to re-run.
#[tokio::test]
async fn test_calculate_matrix_task_changes_resets_failed_tasks_on_state_change() {
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_matrix_node_from_state("node2", vec!["node1"], "items"),
    ]);
    let run = create_test_run(workflow.clone());
    let node2_id = "node2".to_string();

    // --- Initial Setup: Simulating first run with items a, b ---
    let master_task = Task::new(run.id, node2_id.clone(), true);
    let mut task_a = Task::new_matrix(
        run.id,
        node2_id.clone(),
        master_task.id,
        HashMap::from([("id".to_string(), serde_json::Value::String("a".to_string()))]),
    );
    let mut task_b = Task::new_matrix(
        run.id,
        node2_id.clone(),
        master_task.id,
        HashMap::from([("id".to_string(), serde_json::Value::String("b".to_string()))]),
    );

    // Simulate that task_a completed successfully and task_b failed
    task_a.status = TaskStatus::Completed;
    task_b.status = TaskStatus::Failed;

    let initial_tasks = vec![
        Task::new(run.id, "node1".to_string(), false),
        master_task.clone(),
        task_a.clone(),
        task_b.clone(),
    ];

    // State produces the SAME items as before (same hashes)
    // This simulates evaluate-shards being re-triggered and producing the same shards
    let new_state = HashMap::from([("items".to_string(), json!([{"id": "a"}, {"id": "b"}]))]);

    let scheduler = Scheduler::new();

    // --- Calculation ---
    let changes = scheduler
        .calculate_matrix_task_changes(run.id, &run, &initial_tasks, &new_state)
        .await
        .unwrap();

    // --- Assertions ---
    // Should NOT create new tasks (hashes match existing tasks)
    assert_eq!(changes.new_tasks.len(), 0);

    // Should NOT mark any tasks as WontDo (all hashes still exist)
    assert_eq!(changes.tasks_to_mark_wont_do.len(), 0);

    // Should reset task_b (Failed) to Pending so it can be re-run
    assert_eq!(changes.tasks_to_reset_to_pending.len(), 1);
    assert_eq!(changes.tasks_to_reset_to_pending[0], task_b.id);

    // Should update the master task
    assert_eq!(changes.master_tasks_to_update.len(), 1);
    assert_eq!(changes.master_tasks_to_update[0], master_task.id);
}

/// Test that completed tasks are NOT reset when state changes
#[tokio::test]
async fn test_calculate_matrix_task_changes_does_not_reset_completed_tasks() {
    let workflow = create_test_workflow(vec![
        create_basic_node("node1", vec![]),
        create_matrix_node_from_state("node2", vec!["node1"], "items"),
    ]);
    let run = create_test_run(workflow.clone());
    let node2_id = "node2".to_string();

    let master_task = Task::new(run.id, node2_id.clone(), true);
    let mut task_a = Task::new_matrix(
        run.id,
        node2_id.clone(),
        master_task.id,
        HashMap::from([("id".to_string(), serde_json::Value::String("a".to_string()))]),
    );

    // Task is completed - should NOT be reset
    task_a.status = TaskStatus::Completed;

    let initial_tasks = vec![
        Task::new(run.id, "node1".to_string(), false),
        master_task.clone(),
        task_a.clone(),
    ];

    // Same state as before
    let new_state = HashMap::from([("items".to_string(), json!([{"id": "a"}]))]);

    let scheduler = Scheduler::new();

    let changes = scheduler
        .calculate_matrix_task_changes(run.id, &run, &initial_tasks, &new_state)
        .await
        .unwrap();

    // Should NOT create new tasks
    assert_eq!(changes.new_tasks.len(), 0);

    // Should NOT reset completed tasks
    assert_eq!(changes.tasks_to_reset_to_pending.len(), 0);

    // Should NOT mark any tasks as WontDo
    assert_eq!(changes.tasks_to_mark_wont_do.len(), 0);
}
