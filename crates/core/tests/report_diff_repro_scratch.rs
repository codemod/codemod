use butterflow_core::config::{DryRunChange, WorkflowRunConfig};
use butterflow_core::engine::Engine;
use butterflow_core::{Node, Runtime, RuntimeType, Workflow, WorkflowStatus};
use butterflow_models::node::NodeType;
use butterflow_models::step::{SemanticAnalysisConfig, SemanticAnalysisMode, StepAction, UseJSAstGrep};
use butterflow_models::Step;
use butterflow_state::mock_adapter::MockStateAdapter;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn create_test_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    let file_path = dir.join(name);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, content).unwrap();
    file_path
}

async fn wait_for_completion(engine: &Engine, run_id: uuid::Uuid) {
    let start = Instant::now();
    loop {
        let status = engine.get_workflow_status(run_id).await.unwrap();
        if matches!(
            status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Canceled
        ) {
            return;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!("workflow did not complete in time, last status: {status:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn repro_unmodified_file_should_not_produce_diff() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(root) {
  const rootNode = root.root();
  const nodes = rootNode.findAll({ rule: { pattern: 'var $X = $Y' } });
  const edits = nodes.map((node) => {
    const x = node.getMatch('X').text();
    const y = node.getMatch('Y').text();
    return node.replace(`const ${x} = ${y}`);
  });
  return rootNode.commitEdits(edits);
}
"#,
    );

    create_test_file(temp_path, "a.js", "var x = 1;\n");
    create_test_file(temp_path, "b.js", "const y = 2;\n");

    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "transform".to_string(),
            name: "Transform".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
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
                id: Some("jssg".to_string()),
                name: "Convert var to const".to_string(),
                action: StepAction::JSAstGrep(UseJSAstGrep {
                    js_file: "codemod.js".to_string(),
                    base_path: None,
                    include: None,
                    exclude: None,
                    max_threads: Some(1),
                    dry_run: Some(false),
                    language: Some("javascript".to_string()),
                    capabilities: None,
                    semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                        SemanticAnalysisMode::File,
                    )),
                }),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    };

    let collected: Arc<Mutex<Vec<DryRunChange>>> = Arc::new(Mutex::new(Vec::new()));
    let collected_for_cb = collected.clone();

    let mut config = WorkflowRunConfig::default();
    config.execution.target_path = temp_path.to_path_buf();
    config.execution.bundle_path = temp_path.to_path_buf();
    config.output.dry_run_callback = Some(Arc::new(move |change: DryRunChange| {
        collected_for_cb.lock().unwrap().push(change);
    }));

    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, config);

    let run_id = engine
        .run_workflow(workflow, HashMap::new(), Some(temp_path.to_path_buf()), None)
        .await
        .unwrap();

    wait_for_completion(&engine, run_id).await;

    let changes = collected.lock().unwrap().clone();
    eprintln!("=== collected DryRunChange entries: {} ===", changes.len());
    for change in &changes {
        eprintln!(
            "path={} original_len={} new_len={} equal={}",
            change.file_path.display(),
            change.original_content.len(),
            change.new_content.len(),
            change.original_content == change.new_content
        );
        eprintln!("original_bytes={:?}", change.original_content.as_bytes());
        eprintln!("new_bytes=     {:?}", change.new_content.as_bytes());
    }

    let touched_b = changes
        .iter()
        .any(|c| c.file_path.file_name().unwrap() == "b.js");
    assert!(
        !touched_b,
        "b.js was not modified by the codemod and should not appear in diff callback, but got: {:?}",
        changes.iter().map(|c| c.file_path.clone()).collect::<Vec<_>>()
    );

    let a_content = fs::read_to_string(temp_path.join("a.js")).unwrap();
    assert!(a_content.contains("const x"));
}
