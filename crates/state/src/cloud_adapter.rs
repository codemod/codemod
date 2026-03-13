use std::collections::HashMap;

use async_trait::async_trait;
use log::warn;
use reqwest::{header, Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use butterflow_models::{
    DiffOperation, Error, FieldDiff, Result, StateDiff, Task, TaskDiff, WorkflowRun,
    WorkflowRunDiff,
};

use crate::StateAdapter;

/// API request for the sync endpoint
#[derive(Serialize, Clone)]
struct SyncRequest {
    #[serde(rename = "type")]
    request_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_run_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<Uuid>,
    fields: HashMap<String, SyncField>,
}

/// Field for the sync request
#[derive(Serialize, Clone)]
struct SyncField {
    operation: DiffOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
}

/// API response for sync endpoint
#[derive(Deserialize)]
struct SyncResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

const MAX_RETRIES: u32 = 5;
const INITIAL_RETRY_DELAY_MS: u64 = 500;

/// Send an HTTP request with retries on transient failures.
///
/// The `build_request` closure is called on each attempt because
/// `RequestBuilder` is consumed by `send()`.
async fn send_with_retry<F>(mut build_request: F) -> std::result::Result<Response, reqwest::Error>
where
    F: FnMut() -> RequestBuilder,
{
    let mut last_err = None;
    for attempt in 0..=MAX_RETRIES {
        match build_request().send().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                let is_transient = e.is_connect() || e.is_timeout() || e.is_request();
                if !is_transient || attempt == MAX_RETRIES {
                    return Err(e);
                }
                let delay = INITIAL_RETRY_DELAY_MS * 2u64.pow(attempt);
                warn!(
                    "HTTP request failed (attempt {}/{}), retrying in {}ms: {}",
                    attempt + 1,
                    MAX_RETRIES + 1,
                    delay,
                    e
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap())
}

/// API state adapter (sends state updates to an API)
pub struct CloudStateAdapter {
    /// API endpoint
    endpoint: String,

    /// Authentication token
    auth_token: String,

    /// HTTP client
    client: Client,
}

impl CloudStateAdapter {
    /// Create a new API state adapter
    pub fn new(endpoint: String, auth_token: String) -> Self {
        Self {
            endpoint,
            auth_token,
            client: Client::builder()
                .http1_only()
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    /// Get the base URL for the API
    fn get_base_url(&self) -> String {
        self.endpoint.clone() + "/api/butterflow/v1"
    }

    /// Get the authorization header
    fn get_auth_header(&self) -> String {
        format!("Bearer {}", self.auth_token)
    }

    /// Build the sync endpoint URL
    fn get_sync_url(&self) -> String {
        format!("{}/sync", self.get_base_url())
    }

    /// Build a workflow run URL
    fn get_workflow_run_url(&self, workflow_run_id: Uuid) -> String {
        format!("{}/runs/{}", self.get_base_url(), workflow_run_id)
    }

    /// Build a workflow runs list URL
    fn get_workflow_runs_url(&self) -> String {
        format!("{}/runs", self.get_base_url())
    }

    /// Build a task URL
    fn get_task_url(&self, task_id: Uuid) -> String {
        format!("{}/tasks/{}", self.get_base_url(), task_id)
    }

    /// Build a tasks URL for a workflow run
    fn get_tasks_for_workflow_url(&self, workflow_run_id: Uuid) -> String {
        format!("{}/runs/{}/tasks", self.get_base_url(), workflow_run_id)
    }

    /// Build a state URL for a workflow run
    fn get_state_url(&self, workflow_run_id: Uuid) -> String {
        format!("{}/runs/{}/state", self.get_base_url(), workflow_run_id)
    }

    /// Convert FieldDiff to SyncField
    fn convert_field_diff(&self, field_diff: &FieldDiff) -> SyncField {
        SyncField {
            operation: field_diff.operation.clone(),
            value: field_diff.value.clone(),
        }
    }
}

#[async_trait]
impl StateAdapter for CloudStateAdapter {
    async fn save_workflow_run(&mut self, workflow_run: &WorkflowRun) -> Result<()> {
        let workflow_run_value = serde_json::to_value(workflow_run)?;

        if let Value::Object(obj) = workflow_run_value {
            let mut fields = HashMap::new();

            for (key, value) in obj {
                fields.insert(
                    key,
                    SyncField {
                        operation: DiffOperation::Add,
                        value: Some(value),
                    },
                );
            }

            let request = SyncRequest {
                request_type: "workflow_run".to_string(),
                workflow_run_id: Some(workflow_run.id),
                task_id: None,
                fields,
            };

            let response = send_with_retry(|| {
                self.client
                    .post(self.get_sync_url())
                    .header(header::AUTHORIZATION, self.get_auth_header())
                    .json(&request)
            })
            .await?;

            if !response.status().is_success() {
                let error_text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                return Err(Error::Other(format!(
                    "Failed to save workflow run: {error_text}"
                )));
            }

            let sync_response: SyncResponse = response.json().await?;
            if !sync_response.success {
                return Err(Error::Other(format!(
                    "Failed to save workflow run: {}",
                    sync_response
                        .error
                        .unwrap_or_else(|| "Unknown error".to_string())
                )));
            }

            Ok(())
        } else {
            Err(Error::Other("Failed to serialize workflow run".to_string()))
        }
    }

    async fn apply_workflow_run_diff(&mut self, diff: &WorkflowRunDiff) -> Result<()> {
        let mut fields = HashMap::new();
        for (key, field_diff) in &diff.fields {
            fields.insert(key.clone(), self.convert_field_diff(field_diff));
        }

        let request = SyncRequest {
            request_type: "workflow_run".to_string(),
            workflow_run_id: Some(diff.workflow_run_id),
            task_id: None,
            fields,
        };

        let response = send_with_retry(|| {
            self.client
                .post(self.get_sync_url())
                .header(header::AUTHORIZATION, self.get_auth_header())
                .json(&request)
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!(
                "Failed to apply workflow run diff: {error_text}"
            )));
        }

        let sync_response: SyncResponse = response.json().await?;
        if !sync_response.success {
            return Err(Error::Other(format!(
                "Failed to apply workflow run diff: {}",
                sync_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string())
            )));
        }

        Ok(())
    }

    async fn get_workflow_run(&self, workflow_run_id: Uuid) -> Result<WorkflowRun> {
        let response = send_with_retry(|| {
            self.client
                .get(self.get_workflow_run_url(workflow_run_id))
                .header(header::AUTHORIZATION, self.get_auth_header())
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!(
                "Failed to get workflow run: {error_text}"
            )));
        }

        let workflow_run = response.json().await?;
        Ok(workflow_run)
    }

    async fn list_workflow_runs(&self, limit: usize) -> Result<Vec<WorkflowRun>> {
        let response = send_with_retry(|| {
            self.client
                .get(self.get_workflow_runs_url())
                .header(header::AUTHORIZATION, self.get_auth_header())
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!(
                "Failed to list workflow runs: {error_text}"
            )));
        }

        let workflow_runs: Vec<WorkflowRun> = response.json().await?;

        let limited_runs = if workflow_runs.len() > limit {
            workflow_runs.into_iter().take(limit).collect()
        } else {
            workflow_runs
        };

        Ok(limited_runs)
    }

    async fn save_task(&mut self, task: &Task) -> Result<()> {
        let task_value = serde_json::to_value(task)?;

        if let Value::Object(obj) = task_value {
            let mut fields = HashMap::new();

            for (key, value) in obj {
                fields.insert(
                    key,
                    SyncField {
                        operation: DiffOperation::Add,
                        value: Some(value),
                    },
                );
            }

            let request = SyncRequest {
                request_type: "task".to_string(),
                workflow_run_id: None,
                task_id: Some(task.id),
                fields,
            };

            let response = send_with_retry(|| {
                self.client
                    .post(self.get_sync_url())
                    .header(header::AUTHORIZATION, self.get_auth_header())
                    .json(&request)
            })
            .await?;

            if !response.status().is_success() {
                let error_text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                return Err(Error::Other(format!("Failed to save task: {error_text}")));
            }

            let sync_response: SyncResponse = response.json().await?;
            if !sync_response.success {
                return Err(Error::Other(format!(
                    "Failed to save task: {}",
                    sync_response
                        .error
                        .unwrap_or_else(|| "Unknown error".to_string())
                )));
            }

            Ok(())
        } else {
            Err(Error::Other("Failed to serialize task".to_string()))
        }
    }

    async fn apply_task_diff(&mut self, diff: &TaskDiff) -> Result<()> {
        let mut fields = HashMap::new();
        for (key, field_diff) in &diff.fields {
            fields.insert(key.clone(), self.convert_field_diff(field_diff));
        }

        let request = SyncRequest {
            request_type: "task".to_string(),
            workflow_run_id: None,
            task_id: Some(diff.task_id),
            fields,
        };

        let response = send_with_retry(|| {
            self.client
                .post(self.get_sync_url())
                .header(header::AUTHORIZATION, self.get_auth_header())
                .json(&request)
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!(
                "Failed to apply task diff: {error_text}"
            )));
        }

        let sync_response: SyncResponse = response.json().await?;
        if !sync_response.success {
            return Err(Error::Other(format!(
                "Failed to apply task diff: {}",
                sync_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string())
            )));
        }

        Ok(())
    }

    async fn get_task(&self, task_id: Uuid) -> Result<Task> {
        let response = send_with_retry(|| {
            self.client
                .get(self.get_task_url(task_id))
                .header(header::AUTHORIZATION, self.get_auth_header())
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!("Failed to get task: {error_text}")));
        }

        let task = response.json().await?;
        Ok(task)
    }

    async fn get_tasks(&self, workflow_run_id: Uuid) -> Result<Vec<Task>> {
        let response = send_with_retry(|| {
            self.client
                .get(self.get_tasks_for_workflow_url(workflow_run_id))
                .header(header::AUTHORIZATION, self.get_auth_header())
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!("Failed to get tasks: {error_text}")));
        }

        let tasks: Vec<Task> = response.json().await?;
        Ok(tasks)
    }

    async fn update_state(
        &mut self,
        workflow_run_id: Uuid,
        state: HashMap<String, Value>,
    ) -> Result<()> {
        let mut fields = HashMap::new();
        for (key, value) in &state {
            fields.insert(
                key.clone(),
                SyncField {
                    operation: DiffOperation::Update,
                    value: Some(value.clone()),
                },
            );
        }

        let request = SyncRequest {
            request_type: "state".to_string(),
            workflow_run_id: Some(workflow_run_id),
            task_id: None,
            fields,
        };

        let response = send_with_retry(|| {
            self.client
                .post(self.get_sync_url())
                .header(header::AUTHORIZATION, self.get_auth_header())
                .json(&request)
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!(
                "Failed to update state: {error_text}"
            )));
        }

        let sync_response: SyncResponse = response.json().await?;
        if !sync_response.success {
            return Err(Error::Other(format!(
                "Failed to update state: {}",
                sync_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string())
            )));
        }

        Ok(())
    }

    async fn apply_state_diff(&mut self, diff: &StateDiff) -> Result<()> {
        let mut fields = HashMap::new();
        for (key, field_diff) in &diff.fields {
            fields.insert(key.clone(), self.convert_field_diff(field_diff));
        }

        let request = SyncRequest {
            request_type: "state".to_string(),
            workflow_run_id: Some(diff.workflow_run_id),
            task_id: None,
            fields,
        };

        let response = send_with_retry(|| {
            self.client
                .post(self.get_sync_url())
                .header(header::AUTHORIZATION, self.get_auth_header())
                .json(&request)
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!(
                "Failed to apply state diff: {error_text}"
            )));
        }

        let sync_response: SyncResponse = response.json().await?;
        if !sync_response.success {
            return Err(Error::Other(format!(
                "Failed to apply state diff: {}",
                sync_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string())
            )));
        }

        Ok(())
    }

    async fn get_state(&self, workflow_run_id: Uuid) -> Result<HashMap<String, Value>> {
        let response = send_with_retry(|| {
            self.client
                .get(self.get_state_url(workflow_run_id))
                .header(header::AUTHORIZATION, self.get_auth_header())
        })
        .await?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(Error::Other(format!("Failed to get state: {error_text}")));
        }

        let state: HashMap<String, Value> = response.json().await?;
        Ok(state)
    }
}
