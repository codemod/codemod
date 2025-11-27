use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use butterflow_core::utils;
use butterflow_core::NodeType;
use butterflow_models::step::SemanticAnalysisConfig;
use butterflow_models::step::SemanticAnalysisMode;
use butterflow_models::step::StepAction;
use butterflow_models::step::UseAstGrep;
use butterflow_models::step::UseJSAstGrep;
use butterflow_models::strategy::StrategyType;
use butterflow_models::{Error, Node, Step, Strategy, Template, TemplateOutput, Workflow};

#[test]
fn test_parse_workflow_file_yaml() {
    // Create a temporary YAML workflow file
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("workflow.yaml");

    let yaml_content = r#"
version: "1"
nodes:
  - id: node1
    name: Node 1
    steps:
      - id: step1
        name: Step 1
        run: echo "Hello, World!"
"#;

    fs::write(&file_path, yaml_content).unwrap();

    // Parse the workflow file
    let workflow = utils::parse_workflow_file(&file_path).unwrap();

    // Verify the parsed workflow
    assert_eq!(workflow.version, "1");
    assert_eq!(workflow.nodes.len(), 1);
    assert_eq!(workflow.nodes[0].id, "node1");
    assert_eq!(workflow.nodes[0].name, "Node 1");
    assert_eq!(workflow.nodes[0].steps.len(), 1);
    assert_eq!(workflow.nodes[0].steps[0].name, "Step 1");
    if let StepAction::RunScript(script) = &workflow.nodes[0].steps[0].action {
        assert_eq!(script, "echo \"Hello, World!\"");
    } else {
        panic!("Expected StepAction::RunScript with a script");
    }
}

#[test]
fn test_parse_workflow_file_json() {
    // Create a temporary JSON workflow file
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("workflow.json");

    let json_content = r#"
{
  "version": "1",
  "nodes": [
    {
      "id": "node1",
      "name": "Node 1",
      "steps": [
        {
          "id": "step1",
          "name": "Step 1",
          "run": "echo \"Hello, World!\""
        }
      ]
    }
  ]
}
"#;

    fs::write(&file_path, json_content).unwrap();

    // Parse the workflow file
    let workflow = utils::parse_workflow_file(&file_path).unwrap();

    // Verify the parsed workflow
    assert_eq!(workflow.version, "1");
    assert_eq!(workflow.nodes.len(), 1);
    assert_eq!(workflow.nodes[0].id, "node1");
    assert_eq!(workflow.nodes[0].name, "Node 1");
    assert_eq!(workflow.nodes[0].steps.len(), 1);
    assert_eq!(workflow.nodes[0].steps[0].name, "Step 1");
    if let StepAction::RunScript(script) = &workflow.nodes[0].steps[0].action {
        assert_eq!(script, "echo \"Hello, World!\"");
    } else {
        panic!("Expected StepAction::RunScript with a script");
    }
}

#[test]
fn test_parse_workflow_file_invalid() {
    // Create a temporary invalid workflow file
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("workflow.yaml");

    let invalid_content = "This is not a valid YAML or JSON file";

    fs::write(&file_path, invalid_content).unwrap();

    // Parse the workflow file
    let result = utils::parse_workflow_file(&file_path);

    // Verify that parsing fails
    assert!(result.is_err());
    match result {
        Err(Error::WorkflowValidation(_)) => {
            // Expected error
        }
        _ => panic!("Expected WorkflowValidation error"),
    }
}

#[test]
fn test_validate_workflow_valid() {
    // Create a valid workflow
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
        ],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation succeeds
    assert!(result.is_ok());
}

#[test]
fn test_validate_workflow_duplicate_node_id() {
    // Create a workflow with duplicate node IDs
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
            Node {
                id: "node1".to_string(), // Duplicate ID
                name: "Node 2".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
        ],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::WorkflowValidation(msg)) => {
            assert!(msg.contains("Duplicate node ID"));
        }
        _ => panic!("Expected WorkflowValidation error"),
    }
}

#[test]
fn test_validate_workflow_nonexistent_dependency() {
    // Create a workflow with a non-existent dependency
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec!["nonexistent".to_string()], // Non-existent dependency
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::WorkflowValidation(msg)) => {
            assert!(msg.contains("depends on non-existent node"));
        }
        _ => panic!("Expected WorkflowValidation error"),
    }
}

#[test]
fn test_validate_workflow_cyclic_dependency() {
    // Create a workflow with a cyclic dependency
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec!["node2".to_string()],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
        ],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::CyclicDependency(_)) => {
            // Expected error
        }
        _ => panic!("Expected CyclicDependency error"),
    }
}

#[test]
fn test_parse_params() {
    // Test parsing parameters
    let params = vec![
        "key1=value1".to_string(),
        "key2=value2".to_string(),
        "key3=value with spaces".to_string(),
    ];

    let result = utils::parse_params(&params).unwrap();

    // Verify the parsed parameters
    assert_eq!(result.len(), 3);
    assert_eq!(
        result.get("key1"),
        Some(&serde_json::Value::String("value1".to_string()))
    );
    assert_eq!(
        result.get("key2"),
        Some(&serde_json::Value::String("value2".to_string()))
    );
    assert_eq!(
        result.get("key3"),
        Some(&serde_json::Value::String("value with spaces".to_string()))
    );
}

#[test]
fn test_parse_params_invalid() {
    // Test parsing invalid parameters
    let params = vec![
        "key1=value1".to_string(),
        "invalid_param".to_string(), // Missing '='
    ];

    let result = utils::parse_params(&params);

    // Verify that parsing fails
    assert!(result.is_err());
    match result {
        Err(Error::Other(msg)) => {
            assert!(msg.contains("Invalid parameter format"));
        }
        _ => panic!("Expected Other error"),
    }
}

#[test]
fn test_format_duration() {
    // Test formatting durations
    assert_eq!(utils::format_duration(0), "00:00:00");
    assert_eq!(utils::format_duration(30), "00:00:30");
    assert_eq!(utils::format_duration(90), "00:01:30");
    assert_eq!(utils::format_duration(3600), "01:00:00");
    assert_eq!(utils::format_duration(3661), "01:01:01");
    assert_eq!(utils::format_duration(86400), "24:00:00");
    assert_eq!(utils::format_duration(90061), "25:01:01"); // Over 24 hours
    assert_eq!(
        utils::format_duration(u64::MAX),
        format!(
            "{:02}:{:02}:{:02}",
            u64::MAX / 3600,
            (u64::MAX % 3600) / 60,
            u64::MAX % 60
        )
    ); // Edge case: maximum u64 value
}

#[test]
fn test_validate_workflow_duplicate_template_id() {
    // Create a workflow with duplicate template IDs
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![
            Template {
                id: "template1".to_string(),
                name: "Template 1".to_string(),
                description: None,
                inputs: vec![],
                runtime: None,
                steps: vec![],
                outputs: vec![],
                env: HashMap::new(),
            },
            Template {
                id: "template1".to_string(), // Duplicate ID
                name: "Template 2".to_string(),
                description: None,
                inputs: vec![],
                runtime: None,
                steps: vec![],
                outputs: vec![],
                env: HashMap::new(),
            },
        ],
        nodes: vec![],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::WorkflowValidation(msg)) => {
            assert!(msg.contains("Duplicate template ID"));
        }
        _ => panic!("Expected WorkflowValidation error"),
    }
}

#[test]
fn test_validate_workflow_nonexistent_template_reference() {
    // Create a workflow with a non-existent template reference
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![Template {
            id: "template1".to_string(),
            name: "Template 1".to_string(),
            description: None,
            inputs: vec![],
            runtime: None,
            steps: vec![],
            outputs: vec![],
            env: HashMap::new(),
        }],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::UseTemplate(butterflow_models::step::TemplateUse {
                    template: "nonexistent".to_string(), // Non-existent template
                    inputs: HashMap::new(),
                }),
                env: None,
                condition: None,
            }],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::WorkflowValidation(msg)) => {
            assert!(msg.contains("uses non-existent template"));
        }
        _ => panic!("Expected WorkflowValidation error"),
    }
}

#[test]
fn test_validate_workflow_invalid_matrix_strategy() {
    // Create a workflow with an invalid matrix strategy (missing both values and from_state)
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: Some(Strategy {
                r#type: StrategyType::Matrix,
                values: None,
                from_state: None,
            }),
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::WorkflowValidation(msg)) => {
            assert!(msg.contains("Matrix strategy"));
            assert!(msg.contains("requires either 'values' or 'from_state'"));
        }
        _ => panic!("Expected WorkflowValidation error"),
    }
}

#[test]
fn test_validate_workflow_complex_cyclic_dependency() {
    // Create a workflow with a complex cyclic dependency (A -> B -> C -> A)
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node_a".to_string(),
                name: "Node A".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec!["node_c".to_string()],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
            Node {
                id: "node_b".to_string(),
                name: "Node B".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec!["node_a".to_string()],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
            Node {
                id: "node_c".to_string(),
                name: "Node C".to_string(),
                description: None,
                r#type: NodeType::Automatic,
                depends_on: vec!["node_b".to_string()],
                trigger: None,
                strategy: None,
                runtime: None,
                steps: vec![],
                env: HashMap::new(),
            },
        ],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::CyclicDependency(cycle)) => {
            // The cycle detection might not include all nodes in the error message
            // It might just report the specific cycle it found first
            println!("Detected cycle: {cycle}");
            assert!(cycle.contains("node_"));
        }
        _ => panic!("Expected CyclicDependency error"),
    }
}

#[test]
fn test_get_env_vars() {
    // Set a test environment variable
    env::set_var("BUTTERFLOW_TEST_VAR", "test_value");

    // Get environment variables
    let env_vars = utils::get_env_vars();

    // Verify that our test variable is included
    assert_eq!(
        env_vars.get("BUTTERFLOW_TEST_VAR"),
        Some(&"test_value".to_string())
    );

    // Clean up
    env::remove_var("BUTTERFLOW_TEST_VAR");
}

#[test]
fn test_parse_workflow_file_nonexistent() {
    // Try to parse a non-existent file
    let result = utils::parse_workflow_file("nonexistent_file.yaml");

    // Verify that parsing fails
    assert!(result.is_err());
    // The exact error type might vary by platform, so we don't check it specifically
}

#[test]
fn test_parse_workflow_file_empty() {
    // Create a temporary empty file
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("empty.yaml");

    fs::write(&file_path, "").unwrap();

    // Parse the workflow file
    let result = utils::parse_workflow_file(&file_path);

    // Verify that parsing fails
    assert!(result.is_err());
}

#[test]
fn test_parse_params_empty() {
    // Test parsing empty parameters
    let params: Vec<String> = vec![];

    let result = utils::parse_params(&params).unwrap();

    // Verify the parsed parameters
    assert_eq!(result.len(), 0);
}

#[test]
fn test_parse_params_with_empty_value() {
    // Test parsing parameters with empty values
    let params = vec!["key1=".to_string(), "key2=value2".to_string()];

    let result = utils::parse_params(&params).unwrap();

    // Verify the parsed parameters
    assert_eq!(result.len(), 2);
    assert_eq!(
        result.get("key1"),
        Some(&serde_json::Value::String("".to_string()))
    );
    assert_eq!(
        result.get("key2"),
        Some(&serde_json::Value::String("value2".to_string()))
    );
}

#[test]
fn test_parse_params_with_multiple_equals() {
    // Test parsing parameters with multiple equals signs
    let params = vec!["key1=value1=extra".to_string(), "key2=value2".to_string()];

    let result = utils::parse_params(&params).unwrap();

    // Verify the parsed parameters
    assert_eq!(result.len(), 2);
    assert_eq!(
        result.get("key1"),
        Some(&serde_json::Value::String("value1=extra".to_string()))
    );
    assert_eq!(
        result.get("key2"),
        Some(&serde_json::Value::String("value2".to_string()))
    );
}

#[test]
fn test_validate_workflow_self_dependency() {
    // Create a workflow with a self-dependency (node depends on itself)
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec!["node1".to_string()], // Self-dependency
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation fails
    assert!(result.is_err());
    match result {
        Err(Error::CyclicDependency(cycle)) => {
            println!("Detected cycle: {cycle}");
            assert!(cycle.contains("node1"));
        }
        _ => panic!("Expected CyclicDependency error"),
    }
}

#[test]
fn test_validate_workflow_valid_matrix_strategy_with_values() {
    // Create a workflow with a valid matrix strategy (with values)
    let mut values = Vec::new();
    let mut value1 = HashMap::new();
    value1.insert(
        "region".to_string(),
        serde_json::to_value("us-east").unwrap(),
    );
    let mut value2 = HashMap::new();
    value2.insert(
        "region".to_string(),
        serde_json::to_value("us-west").unwrap(),
    );
    values.push(value1);
    values.push(value2);

    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: Some(Strategy {
                r#type: StrategyType::Matrix,
                values: Some(values),
                from_state: None,
            }),
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation succeeds
    assert!(result.is_ok());
}

#[test]
fn test_validate_workflow_valid_matrix_strategy_with_from_state() {
    // Create a workflow with a valid matrix strategy (with from_state)
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: Some(Strategy {
                r#type: StrategyType::Matrix,
                values: None,
                from_state: Some("testState".to_string()),
            }),
            runtime: None,
            steps: vec![],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation succeeds
    assert!(result.is_ok());
}

#[test]
fn test_validate_workflow_with_template_outputs() {
    // Create a workflow with a template that has outputs
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![Template {
            id: "template1".to_string(),
            name: "Template 1".to_string(),
            description: None,
            inputs: vec![],
            runtime: None,
            steps: vec![],
            outputs: vec![TemplateOutput {
                name: "output1".to_string(),
                value: "${result}".to_string(),
                description: None,
            }],
            env: HashMap::new(),
        }],
        nodes: vec![],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation succeeds
    assert!(result.is_ok());
}

#[test]
fn test_validate_workflow_with_step_env_vars() {
    // Create a workflow with a step that has environment variables
    let mut step_env = HashMap::new();
    step_env.insert("STEP_VAR".to_string(), "step_value".to_string());

    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::RunScript("echo $STEP_VAR".to_string()),
                env: Some(step_env),
                condition: None,
            }],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new(""));

    // Verify that validation succeeds
    assert!(result.is_ok());
}

#[test]
fn test_validate_workflow_with_bundle_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bundle_path = temp_dir.path().join("bundle");
    fs::create_dir_all(&bundle_path).unwrap();

    let ast_grep_file = bundle_path.join("config.yml");
    fs::write(
        &ast_grep_file,
        "id: replace-console-log
language: javascript
rule:
  any:
    - pattern: console.log($ARG)
    - pattern: console.debug($ARG)
fix:
  logger.log($ARG)
",
    )
    .unwrap();

    let js_file = bundle_path.join("codemod.ts");
    fs::write(&js_file, "console.log('Hello, World!');").unwrap();

    // Create a workflow with a bundle path
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![
                Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::AstGrep(UseAstGrep {
                        include: Some(vec!["**/*.ts".to_string()]),
                        exclude: Some(vec!["**/node_modules/**".to_string()]),
                        base_path: Some(".".to_string()),
                        config_file: "config.yml".to_string(),
                        allow_dirty: Some(false),
                        max_threads: None,
                    }),
                    env: None,
                    condition: None,
                },
                Step {
                    id: Some("step-2".to_string()),
                    name: "Step 2".to_string(),
                    action: StepAction::JSAstGrep(UseJSAstGrep {
                        js_file: "codemod.ts".to_string(),
                        include: Some(vec!["**/*.ts".to_string()]),
                        exclude: Some(vec!["**/node_modules/**".to_string()]),
                        base_path: Some(".".to_string()),
                        max_threads: None,
                        dry_run: Some(true),
                        language: Some("typescript".to_string()),
                        capabilities: None,
                        semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                            SemanticAnalysisMode::File,
                        )),
                    }),
                    env: None,
                    condition: None,
                },
            ],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, bundle_path.as_path());

    // Verify that validation succeeds
    assert!(result.is_ok());
}

#[test]
fn test_validate_workflow_with_invalid_config_path() {
    // Create a workflow with an invalid config path
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: None,
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::AstGrep(UseAstGrep {
                    config_file: "config.yaml".to_string(),
                    include: Some(vec!["**/*.ts".to_string()]),
                    exclude: Some(vec!["**/node_modules/**".to_string()]),
                    base_path: Some(".".to_string()),
                    allow_dirty: Some(false),
                    max_threads: None,
                }),
                env: None,
                condition: None,
            }],
            env: HashMap::new(),
        }],
    };

    // Validate the workflow
    let result = utils::validate_workflow(&workflow, Path::new("."));

    // Verify that validation fails
    assert!(result.is_err());
}

#[test]
fn test_parse_workflow_file_complex() {
    // Create a temporary YAML workflow file with complex structure
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("complex.yaml");

    let yaml_content = r#"
version: "1"
state:
  schema:
    testState:
      type: array
      items:
        type: object
        properties:
          key:
            type: string
templates:
  - id: template1
    name: Template 1
    steps:
      - id: step1
        name: Step 1
        run: echo "Template step"
nodes:
  - id: node1
    name: Node 1
    type: automatic
    steps:
      - id: step1
        name: Step 1
        run: echo "Hello, World!"
      - id: step2
        name: Step 2
        use:
          template: template1
          inputs:
            param1: value1
  - id: node2
    name: Node 2
    type: manual
    depends_on:
      - node1
    strategy:
      type: matrix
      values:
        - region: us-east
        - region: us-west
    steps:
      - id: step1
        name: Step 1
        run: echo "Processing region"
"#;

    fs::write(&file_path, yaml_content).unwrap();

    // Parse the workflow file
    let workflow = utils::parse_workflow_file(&file_path).unwrap();

    // Verify the parsed workflow
    assert_eq!(workflow.version, "1");
    assert_eq!(workflow.nodes.len(), 2);
    assert_eq!(workflow.templates.len(), 1);

    // Check first node
    assert_eq!(workflow.nodes[0].id, "node1");
    assert_eq!(workflow.nodes[0].name, "Node 1");
    assert_eq!(workflow.nodes[0].r#type, NodeType::Automatic);
    assert_eq!(workflow.nodes[0].steps.len(), 2);

    // Check second node
    assert_eq!(workflow.nodes[1].id, "node2");
    assert_eq!(workflow.nodes[1].name, "Node 2");
    assert_eq!(workflow.nodes[1].r#type, NodeType::Manual);
    assert_eq!(workflow.nodes[1].depends_on, vec!["node1"]);
    assert!(workflow.nodes[1].strategy.is_some());

    // Check template
    assert_eq!(workflow.templates[0].id, "template1");
    assert_eq!(workflow.templates[0].name, "Template 1");
    assert_eq!(workflow.templates[0].steps.len(), 1);
}
