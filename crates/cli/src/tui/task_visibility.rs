//! Which workflow tasks appear in the task list.
//!
//! Matrix nodes use a "master" task plus one task per matrix combination. The TUI
//! historically hid all master tasks to avoid duplicate rows once child tasks exist.
//! For `strategy.matrix.from_state`, only the master exists until state (e.g. shards)
//! is written and the scheduler materializes child tasks — hiding the master made
//! manual matrix nodes invisible until children appeared.

use butterflow_models::{Task, TaskStatus};

/// Whether `task` should be shown as a row in the workflow task list.
pub(crate) fn task_visible_in_list(task: &Task, all_tasks: &[Task]) -> bool {
    if !task.is_master {
        return true;
    }
    let has_children = all_tasks
        .iter()
        .any(|t| t.master_task_id == Some(task.id));
    !has_children
}

pub(crate) fn awaiting_trigger_visible(tasks: &[Task]) -> bool {
    tasks.iter().any(|task| {
        task.status == TaskStatus::AwaitingTrigger && task_visible_in_list(task, tasks)
    })
}

pub(crate) fn failed_visible(tasks: &[Task]) -> bool {
    tasks.iter().any(|task| {
        task.status == TaskStatus::Failed && task_visible_in_list(task, tasks)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn base_task(
        workflow_run_id: Uuid,
        node_id: &str,
        is_master: bool,
        master_task_id: Option<Uuid>,
    ) -> Task {
        Task {
            id: Uuid::new_v4(),
            workflow_run_id,
            node_id: node_id.to_string(),
            status: TaskStatus::Blocked,
            is_master,
            master_task_id,
            matrix_values: None,
            started_at: Some(Utc::now()),
            ended_at: None,
            error: None,
            logs: Vec::new(),
        }
    }

    #[test]
    fn non_master_always_visible() {
        let run = Uuid::new_v4();
        let t = base_task(run, "evaluate-shards", false, None);
        assert!(task_visible_in_list(&t, &[t.clone()]));
    }

    #[test]
    fn matrix_master_visible_until_children_exist() {
        let run = Uuid::new_v4();
        let master = base_task(run, "apply-transforms", true, None);
        assert!(task_visible_in_list(&master, &[master.clone()]));

        let child = {
            let mut c = base_task(run, "apply-transforms", false, Some(master.id));
            c.matrix_values = Some(
                std::collections::HashMap::from([("name".to_string(), serde_json::json!("a"))]),
            );
            c
        };
        assert!(!task_visible_in_list(&master, &[master.clone(), child.clone()]));
        assert!(task_visible_in_list(&child, &[master.clone(), child.clone()]));
    }
}
