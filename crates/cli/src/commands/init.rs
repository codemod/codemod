use anyhow::{anyhow, Result};
use clap::Args;
use console::{style, Emoji};
use inquire::{Confirm, Select, Text};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Args, Debug)]
pub struct Command {
    /// Project directory name
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Project name (defaults to directory name)
    #[arg(long)]
    name: Option<String>,

    /// Git repository URL
    #[arg(long)]
    git_repository_url: Option<String>,

    /// Project type
    #[arg(long)]
    project_type: Option<ProjectType>,

    /// Package manager
    #[arg(long)]
    package_manager: Option<String>,

    /// Target language
    #[arg(long)]
    language: Option<String>,

    /// Project description
    #[arg(long)]
    description: Option<String>,

    /// Author name and email
    #[arg(long)]
    author: Option<String>,

    /// License
    #[arg(long)]
    license: Option<String>,

    /// Make package private
    #[arg(long)]
    private: bool,

    /// Overwrite existing files
    #[arg(long)]
    force: bool,

    /// Use defaults without prompts
    #[arg(long)]
    no_interactive: bool,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
enum ProjectType {
    /// JavaScript ast-grep codemod
    AstGrepJs,
    /// Multi-step workflow: Shell + YAML + jssg
    Hybrid,
    /// Shell command workflow codemod (legacy)
    Shell,
    /// YAML ast-grep codemod (legacy)
    AstGrepYaml,
}

struct ProjectConfig {
    name: String,
    description: String,
    author: String,
    license: String,
    project_type: ProjectType,
    language: String,
    private: bool,
    package_manager: Option<String>,
    git_repository_url: Option<String>,
}

// Template constants using include_str!
const CODEMOD_TEMPLATE: &str = include_str!("../templates/codemod.yaml");
const SHELL_WORKFLOW_TEMPLATE: &str = include_str!("../templates/shell/workflow.yaml");
const JS_ASTGREP_WORKFLOW_TEMPLATE: &str = include_str!("../templates/js-astgrep/workflow.yaml");
const ASTGREP_YAML_WORKFLOW_TEMPLATE: &str =
    include_str!("../templates/astgrep-yaml/workflow.yaml");
const HYBRID_WORKFLOW_TEMPLATE: &str = include_str!("../templates/hybrid/workflow.yaml");
const GITIGNORE_TEMPLATE: &str = include_str!("../templates/common/.gitignore");
const README_TEMPLATE: &str = include_str!("../templates/common/README.md");

// Shell project templates
const SHELL_SETUP_SCRIPT: &str = include_str!("../templates/shell/scripts/setup.sh");
const SHELL_TRANSFORM_SCRIPT: &str = include_str!("../templates/shell/scripts/transform.sh");
const SHELL_CLEANUP_SCRIPT: &str = include_str!("../templates/shell/scripts/cleanup.sh");

// JS ast-grep project templates
const JS_PACKAGE_JSON_TEMPLATE: &str = include_str!("../templates/js-astgrep/package.json");
const JS_APPLY_SCRIPT_FOR_JAVASCRIPT: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.ts.ts");
const JS_APPLY_SCRIPT_FOR_PYTHON: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.py.ts");
const JS_APPLY_SCRIPT_FOR_RUST: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.rs.ts");
const JS_APPLY_SCRIPT_FOR_GO: &str = include_str!("../templates/js-astgrep/scripts/codemod.go.ts");
const JS_APPLY_SCRIPT_FOR_JAVA: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.java.ts");
const JS_APPLY_SCRIPT_FOR_HTML: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.html.ts");
const JS_APPLY_SCRIPT_FOR_CSS: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.css.ts");
const JS_APPLY_SCRIPT_FOR_KOTLIN: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.kt.ts");
const JS_APPLY_SCRIPT_FOR_ANGULAR: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.angular.ts");
const JS_APPLY_SCRIPT_FOR_CSHARP: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.cs.ts");
const JS_APPLY_SCRIPT_FOR_CPP: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.cpp.ts");
const JS_APPLY_SCRIPT_FOR_C: &str = include_str!("../templates/js-astgrep/scripts/codemod.c.ts");
const JS_APPLY_SCRIPT_FOR_PHP: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.php.ts");
const JS_APPLY_SCRIPT_FOR_RUBY: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.rb.ts");
const JS_APPLY_SCRIPT_FOR_ELIXIR: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.ex.ts");
const JS_APPLY_SCRIPT_FOR_JSON: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.json.ts");
const JS_APPLY_SCRIPT_FOR_YAML: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.yaml.ts");
const JS_TSCONFIG_TEMPLATE: &str = include_str!("../templates/js-astgrep/tsconfig.json");

// fixtures
const JS_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.js");
const JS_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.js");
const GO_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.go");
const GO_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.go");
const PYTHON_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.py");
const PYTHON_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.py");
const RUST_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.rs");
const RUST_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.rs");
const JAVA_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.java");
const JAVA_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.java");
const HTML_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.html");
const HTML_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.html");
const CSS_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.css");
const CSS_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.css");
const KOTLIN_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.kt");
const KOTLIN_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.kt");
const ANGULAR_TEST_INPUT: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/input-angular.ts");
const ANGULAR_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected-angular.ts");
const CSHARP_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.cs");
const CSHARP_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.cs");
const CPP_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.cpp");
const CPP_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.cpp");
const C_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.c");
const C_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.c");
const PHP_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.php");
const PHP_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.php");
const RUBY_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.rb");
const RUBY_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.rb");
const ELIXIR_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.ex");
const ELIXIR_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.ex");
const JSON_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.json");
const JSON_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.json");
const YAML_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.yaml");
const YAML_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.yaml");

// ast-grep YAML project templates
const ASTGREP_PATTERNS_FOR_JAVASCRIPT: &str =
    include_str!("../templates/astgrep-yaml/rules/config.ts.yml");
const ASTGREP_PATTERNS_FOR_PYTHON: &str =
    include_str!("../templates/astgrep-yaml/rules/config.py.yml");
const ASTGREP_PATTERNS_FOR_RUST: &str =
    include_str!("../templates/astgrep-yaml/rules/config.rs.yml");
const ASTGREP_PATTERNS_FOR_GO: &str = include_str!("../templates/astgrep-yaml/rules/config.go.yml");
const ASTGREP_PATTERNS_FOR_JAVA: &str =
    include_str!("../templates/astgrep-yaml/rules/config.java.yml");
const ASTGREP_PATTERNS_FOR_HTML: &str =
    include_str!("../templates/astgrep-yaml/rules/config.html.yml");
const ASTGREP_PATTERNS_FOR_CSS: &str =
    include_str!("../templates/astgrep-yaml/rules/config.css.yml");
const ASTGREP_PATTERNS_FOR_KOTLIN: &str =
    include_str!("../templates/astgrep-yaml/rules/config.kt.yml");
const ASTGREP_PATTERNS_FOR_ANGULAR: &str =
    include_str!("../templates/astgrep-yaml/rules/config.angular.yml");
const ASTGREP_PATTERNS_FOR_CSHARP: &str =
    include_str!("../templates/astgrep-yaml/rules/config.cs.yml");
const ASTGREP_PATTERNS_FOR_CPP: &str =
    include_str!("../templates/astgrep-yaml/rules/config.cpp.yml");
const ASTGREP_PATTERNS_FOR_C: &str = include_str!("../templates/astgrep-yaml/rules/config.c.yml");
const ASTGREP_PATTERNS_FOR_PHP: &str =
    include_str!("../templates/astgrep-yaml/rules/config.php.yml");
const ASTGREP_PATTERNS_FOR_RUBY: &str =
    include_str!("../templates/astgrep-yaml/rules/config.rb.yml");
const ASTGREP_PATTERNS_FOR_ELIXIR: &str =
    include_str!("../templates/astgrep-yaml/rules/config.ex.yml");
const ASTGREP_PATTERNS_FOR_JSON: &str =
    include_str!("../templates/astgrep-yaml/rules/config.json.yml");
const ASTGREP_PATTERNS_FOR_YAML: &str =
    include_str!("../templates/astgrep-yaml/rules/config.yaml.yml");

static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static CHECKMARK: Emoji<'_, '_> = Emoji("✓ ", "");

pub fn handler(args: &Command) -> Result<()> {
    let git_repository_url = args.git_repository_url.clone();

    let (project_path, project_name, git_repository_url) = if args.no_interactive {
        let project_path = match args.path.clone() {
            Some(path) => path,
            None => return Err(anyhow!("Path argument is required")),
        };

        let project_name = match args.name.clone() {
            Some(name) => name,
            None => {
                let file_name = project_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "Could not determine project name from path {}",
                            project_path.display()
                        )
                    })?;
                file_name.to_string()
            }
        };

        (project_path, project_name, git_repository_url)
    } else {
        // Interactive mode - ask for path if not provided
        let project_path = if let Some(path) = &args.path {
            path.clone()
        } else {
            let path_str = Text::new("Project directory:")
                .with_default("my-codemod")
                .prompt()?;
            PathBuf::from(path_str)
        };

        let project_name = args.name.clone().unwrap_or_else(|| {
            project_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("my-codemod")
                .to_string()
        });

        (project_path, project_name, git_repository_url)
    };

    if project_path.exists() && !args.force {
        return Err(anyhow!(
            "Directory already exists: {}. Use --force to overwrite.",
            project_path.display()
        ));
    }

    let config = if args.no_interactive {
        let project_type = args
            .project_type
            .clone()
            .ok_or_else(|| anyhow!("Project type is required --project-type"))?;
        let normalized_project_type = match project_type {
            ProjectType::Shell | ProjectType::AstGrepYaml => {
                println!(
                    "{} Deprecated project type selected; scaffolding a Hybrid (Shell + YAML + jssg) package",
                    style("ℹ").cyan(),
                );
                ProjectType::Hybrid
            }
            other => other,
        };
        let package_manager = match (&normalized_project_type, args.package_manager.clone()) {
            (ProjectType::AstGrepJs, Some(pm)) | (ProjectType::Hybrid, Some(pm)) => Some(pm),
            (ProjectType::AstGrepJs, None) | (ProjectType::Hybrid, None) => {
                return Err(anyhow!(
                    "--package-manager is required when --project-type is ast-grep-js or hybrid"
                ));
            }
            _ => None,
        };
        ProjectConfig {
            name: project_name,
            description: args
                .description
                .clone()
                .ok_or_else(|| anyhow!("Description is required --description"))?,
            author: args
                .author
                .clone()
                .ok_or_else(|| anyhow!("Author is required --author"))?,
            license: args
                .license
                .clone()
                .ok_or_else(|| anyhow!("License is required --license"))?,
            project_type: normalized_project_type.clone(),
            language: args
                .language
                .clone()
                .ok_or_else(|| anyhow!("Language is required --language"))?,
            private: args.private,
            package_manager,
            git_repository_url,
        }
    } else {
        interactive_setup(&project_name, args)?
    };

    create_project(&project_path, &config)?;

    // Run post init commands
    run_post_init_commands(&project_path, &config, args.no_interactive)?;

    let project_absolute_path = project_path.canonicalize()?;

    print_next_steps(&project_absolute_path, &config)?;

    Ok(())
}

fn interactive_setup(project_name: &str, args: &Command) -> Result<ProjectConfig> {
    println!(
        "{} {}",
        ROCKET,
        style("Creating a new codemod project").bold()
    );
    println!();

    // Project type selection
    let project_type = if let Some(pt) = &args.project_type {
        pt.clone()
    } else {
        select_project_type()?
    };

    // Language selection
    let language = if let Some(lang) = &args.language {
        lang.clone()
    } else {
        select_language()?
    };

    // Project details
    let name = if args.name.is_some() {
        args.name.clone().unwrap()
    } else {
        Text::new("Project name:")
            .with_default(project_name)
            .prompt()?
    };

    let git_repository_url = if let Some(url) = &args.git_repository_url {
        url.clone()
    } else {
        Text::new("Git repository URL:")
            .with_validator(|input: &str| {
                if input.is_empty() || input.starts_with("https://") {
                    Ok(inquire::validator::Validation::Valid)
                } else {
                    Ok(inquire::validator::Validation::Invalid(
                        "Please enter a valid Git URL (must start with 'https://') or leave empty to skip."
                            .into(),
                    ))
                }
            })
            .prompt()?
    };

    let description = if let Some(desc) = &args.description {
        desc.clone()
    } else {
        Text::new("Description:")
            .with_default("Transform legacy code patterns")
            .prompt()?
    };

    let author = if let Some(auth) = &args.author {
        auth.clone()
    } else {
        Text::new("Author:")
            .with_default("Author <author@example.com>")
            .prompt()?
    };

    let license = if let Some(lic) = &args.license {
        lic.clone()
    } else {
        Text::new("License:").with_default("MIT").prompt()?
    };

    let private = if args.private {
        true
    } else {
        Confirm::new("Private package?")
            .with_default(false)
            .prompt()?
    };

    Ok(ProjectConfig {
        name,
        description,
        author,
        license,
        project_type,
        language,
        private,
        package_manager: args.package_manager.clone(),
        git_repository_url: if git_repository_url.is_empty() {
            None
        } else {
            Some(git_repository_url)
        },
    })
}

fn select_project_type() -> Result<ProjectType> {
    let options = vec![
        "jssg codemod (covers most use cases)",
        "multi-step workflow (shell command, YAML & jssg)",
    ];

    let selection =
        Select::new("What type of codemod would you like to create?", options).prompt()?;

    match selection {
        "jssg codemod (covers most use cases)" => Ok(ProjectType::AstGrepJs),
        "multi-step workflow (shell command, YAML & jssg)" => Ok(ProjectType::Hybrid),
        _ => Ok(ProjectType::AstGrepJs), // Default fallback
    }
}

fn select_language() -> Result<String> {
    let options = vec![
        "JavaScript/TypeScript",
        "Python",
        "Rust",
        "Go",
        "Java",
        "HTML",
        "CSS",
        "Kotlin",
        "Angular",
        "C#",
        "C++",
        "C",
        "PHP",
        "Ruby",
        "Elixir",
        "Json",
        "Yaml",
        "Other",
    ];

    let selection = Select::new("Which language would you like to target?", options).prompt()?;

    let language = match selection {
        "JavaScript/TypeScript" => "typescript",
        "Python" => "python",
        "Rust" => "rust",
        "Go" => "go",
        "Java" => "java",
        "HTML" => "html",
        "CSS" => "css",
        "Kotlin" => "kotlin",
        "Angular" => "angular",
        "C#" => "csharp",
        "C++" => "cpp",
        "C" => "c",
        "PHP" => "php",
        "Ruby" => "ruby",
        "Elixir" => "elixir",
        "Json" => "json",
        "Yaml" => "yaml",
        "Other" => {
            let custom = Text::new("Enter language name:").prompt()?;
            return Ok(custom);
        }
        _ => "typescript",
    };

    Ok(language.to_string())
}

fn create_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create project directory
    fs::create_dir_all(project_path)?;

    // Create codemod.yaml
    create_manifest(project_path, config)?;

    // Create workflow.yaml
    create_workflow(project_path, config)?;

    // Create project-specific structure
    match config.project_type {
        ProjectType::Shell => create_shell_project(project_path, config)?,
        ProjectType::AstGrepJs => create_js_astgrep_project(project_path, config)?,
        ProjectType::AstGrepYaml => create_astgrep_yaml_project(project_path, config)?,
        ProjectType::Hybrid => create_hybrid_project(project_path, config)?,
    }

    // Create common files
    create_gitignore(project_path)?;
    create_readme(project_path, config)?;

    info!("✓ Created {} project", config.name);
    Ok(())
}

fn create_manifest(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let repository_line = if let Some(url) = &config.git_repository_url {
        format!("repository: \"{}\"", url)
    } else {
        String::new()
    };

    let manifest_content = CODEMOD_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{author}", &config.author)
        .replace("{license}", &config.license)
        .replace("{language}", &config.language)
        .replace(
            "{access}",
            if config.private { "private" } else { "public" },
        )
        .replace(
            "{visibility}",
            if config.private { "private" } else { "public" },
        )
        .replace("{repository}", &repository_line);

    fs::write(project_path.join("codemod.yaml"), manifest_content)?;
    Ok(())
}

fn create_workflow(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let workflow_content = match config.project_type {
        ProjectType::Shell => SHELL_WORKFLOW_TEMPLATE,
        ProjectType::AstGrepJs => JS_ASTGREP_WORKFLOW_TEMPLATE,
        ProjectType::AstGrepYaml => ASTGREP_YAML_WORKFLOW_TEMPLATE,
        ProjectType::Hybrid => HYBRID_WORKFLOW_TEMPLATE,
    }
    .replace("{language}", &config.language);

    fs::write(project_path.join("workflow.yaml"), workflow_content)?;
    Ok(())
}

fn create_shell_project(project_path: &Path, _config: &ProjectConfig) -> Result<()> {
    // Create scripts directory
    let scripts_dir = project_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    // Create setup script
    fs::write(scripts_dir.join("setup.sh"), SHELL_SETUP_SCRIPT)?;

    // Create transform script
    fs::write(scripts_dir.join("transform.sh"), SHELL_TRANSFORM_SCRIPT)?;

    // Create cleanup script
    fs::write(scripts_dir.join("cleanup.sh"), SHELL_CLEANUP_SCRIPT)?;

    Ok(())
}

fn create_js_astgrep_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create package.json
    let package_json = JS_PACKAGE_JSON_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description);

    fs::write(project_path.join("package.json"), package_json)?;

    // Create scripts directory
    let scripts_dir = project_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    let codemod_script = match config.language.as_str() {
        "javascript" | "typescript" => JS_APPLY_SCRIPT_FOR_JAVASCRIPT.to_string(),
        "python" => JS_APPLY_SCRIPT_FOR_PYTHON.to_string(),
        "rust" => JS_APPLY_SCRIPT_FOR_RUST.to_string(),
        "go" => JS_APPLY_SCRIPT_FOR_GO.to_string(),
        "java" => JS_APPLY_SCRIPT_FOR_JAVA.to_string(),
        "html" => JS_APPLY_SCRIPT_FOR_HTML.to_string(),
        "css" => JS_APPLY_SCRIPT_FOR_CSS.to_string(),
        "kotlin" => JS_APPLY_SCRIPT_FOR_KOTLIN.to_string(),
        "angular" => JS_APPLY_SCRIPT_FOR_ANGULAR.to_string(),
        "csharp" => JS_APPLY_SCRIPT_FOR_CSHARP.to_string(),
        "cpp" => JS_APPLY_SCRIPT_FOR_CPP.to_string(),
        "c" => JS_APPLY_SCRIPT_FOR_C.to_string(),
        "php" => JS_APPLY_SCRIPT_FOR_PHP.to_string(),
        "ruby" => JS_APPLY_SCRIPT_FOR_RUBY.to_string(),
        "elixir" => JS_APPLY_SCRIPT_FOR_ELIXIR.to_string(),
        "json" => JS_APPLY_SCRIPT_FOR_JSON.to_string(),
        "yaml" => JS_APPLY_SCRIPT_FOR_YAML.to_string(),
        _ => JS_APPLY_SCRIPT_FOR_JAVASCRIPT.to_string(),
    };
    fs::write(scripts_dir.join("codemod.ts"), codemod_script.as_str())?;

    // Create tsconfig.json
    fs::write(project_path.join("tsconfig.json"), JS_TSCONFIG_TEMPLATE)?;

    // Create tests
    create_js_tests(project_path, config)?;

    Ok(())
}

fn create_astgrep_yaml_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create rules directory
    let rules_dir = project_path.join("rules");
    fs::create_dir_all(&rules_dir)?;

    let config_file = match config.language.as_str() {
        "javascript" | "typescript" => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
        "python" => ASTGREP_PATTERNS_FOR_PYTHON,
        "rust" => ASTGREP_PATTERNS_FOR_RUST,
        "go" => ASTGREP_PATTERNS_FOR_GO,
        "java" => ASTGREP_PATTERNS_FOR_JAVA,
        "html" => ASTGREP_PATTERNS_FOR_HTML,
        "css" => ASTGREP_PATTERNS_FOR_CSS,
        "kotlin" => ASTGREP_PATTERNS_FOR_KOTLIN,
        "angular" => ASTGREP_PATTERNS_FOR_ANGULAR,
        "csharp" => ASTGREP_PATTERNS_FOR_CSHARP,
        "cpp" => ASTGREP_PATTERNS_FOR_CPP,
        "c" => ASTGREP_PATTERNS_FOR_C,
        "php" => ASTGREP_PATTERNS_FOR_PHP,
        "ruby" => ASTGREP_PATTERNS_FOR_RUBY,
        "elixir" => ASTGREP_PATTERNS_FOR_ELIXIR,
        "json" => ASTGREP_PATTERNS_FOR_JSON,
        "yaml" => ASTGREP_PATTERNS_FOR_YAML,
        _ => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
    };
    fs::write(rules_dir.join("config.yml"), config_file)?;

    // Create tests directory
    let tests_dir = project_path.join("tests");
    fs::create_dir_all(tests_dir.join("input"))?;
    fs::create_dir_all(tests_dir.join("expected"))?;

    Ok(())
}

fn create_hybrid_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create scripts directory
    let scripts_dir = project_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    // Copy shell scripts
    fs::write(scripts_dir.join("setup.sh"), SHELL_SETUP_SCRIPT)?;
    fs::write(scripts_dir.join("transform.sh"), SHELL_TRANSFORM_SCRIPT)?;
    fs::write(scripts_dir.join("cleanup.sh"), SHELL_CLEANUP_SCRIPT)?;

    // Copy jssg codemod script
    let codemod_script = match config.language.as_str() {
        "javascript" | "typescript" => JS_APPLY_SCRIPT_FOR_JAVASCRIPT,
        "python" => JS_APPLY_SCRIPT_FOR_PYTHON,
        "rust" => JS_APPLY_SCRIPT_FOR_RUST,
        "go" => JS_APPLY_SCRIPT_FOR_GO,
        "java" => JS_APPLY_SCRIPT_FOR_JAVA,
        "html" => JS_APPLY_SCRIPT_FOR_HTML,
        "css" => JS_APPLY_SCRIPT_FOR_CSS,
        "kotlin" => JS_APPLY_SCRIPT_FOR_KOTLIN,
        "angular" => JS_APPLY_SCRIPT_FOR_ANGULAR,
        "csharp" => JS_APPLY_SCRIPT_FOR_CSHARP,
        "cpp" => JS_APPLY_SCRIPT_FOR_CPP,
        "c" => JS_APPLY_SCRIPT_FOR_C,
        "php" => JS_APPLY_SCRIPT_FOR_PHP,
        "ruby" => JS_APPLY_SCRIPT_FOR_RUBY,
        "elixir" => JS_APPLY_SCRIPT_FOR_ELIXIR,
        "json" => JS_APPLY_SCRIPT_FOR_JSON,
        "yaml" => JS_APPLY_SCRIPT_FOR_YAML,
        _ => JS_APPLY_SCRIPT_FOR_JAVASCRIPT,
    };
    fs::write(scripts_dir.join("codemod.ts"), codemod_script)?;

    // Create rules directory
    let rules_dir = project_path.join("rules");
    fs::create_dir_all(&rules_dir)?;

    let config_file = match config.language.as_str() {
        "javascript" | "typescript" => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
        "python" => ASTGREP_PATTERNS_FOR_PYTHON,
        "rust" => ASTGREP_PATTERNS_FOR_RUST,
        "go" => ASTGREP_PATTERNS_FOR_GO,
        "java" => ASTGREP_PATTERNS_FOR_JAVA,
        "html" => ASTGREP_PATTERNS_FOR_HTML,
        "css" => ASTGREP_PATTERNS_FOR_CSS,
        "kotlin" => ASTGREP_PATTERNS_FOR_KOTLIN,
        "angular" => ASTGREP_PATTERNS_FOR_ANGULAR,
        "csharp" => ASTGREP_PATTERNS_FOR_CSHARP,
        "cpp" => ASTGREP_PATTERNS_FOR_CPP,
        "c" => ASTGREP_PATTERNS_FOR_C,
        "php" => ASTGREP_PATTERNS_FOR_PHP,
        "ruby" => ASTGREP_PATTERNS_FOR_RUBY,
        "elixir" => ASTGREP_PATTERNS_FOR_ELIXIR,
        "json" => ASTGREP_PATTERNS_FOR_JSON,
        "yaml" => ASTGREP_PATTERNS_FOR_YAML,
        _ => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
    };
    fs::write(rules_dir.join("config.yml"), config_file)?;

    // Create tests directory
    let tests_dir = project_path.join("tests");
    fs::create_dir_all(tests_dir.join("fixtures"))?;

    if config.language == "javascript" || config.language == "typescript" {
        fs::write(tests_dir.join("fixtures").join("input.js"), JS_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.js"),
            JS_TEST_EXPECTED,
        )?;
    }

    // Create package.json and tsconfig.json at project root
    let package_json_content = format!(
        r#"{{
  "name": "{}",
  "version": "1.0.0",
  "description": "{}",
  "main": "scripts/codemod.ts",
  "scripts": {{
    "test": "node scripts/codemod.ts"
  }},
  "dependencies": {{
    "codemod:ast-grep": "latest"
  }},
  "devDependencies": {{
    "@types/node": "^20.0.0",
    "typescript": "^5.0.0"
  }}
}}"#,
        config.name, config.description
    );

    let tsconfig_content = r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "lib": ["ES2020"],
    "outDir": "./dist",
    "rootDir": "./",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "moduleResolution": "node",
    "allowSyntheticDefaultImports": true,
    "experimentalDecorators": true,
    "emitDecoratorMetadata": true
  },
  "include": ["scripts/**/*"],
  "exclude": ["node_modules", "dist"]
}"#;

    fs::write(project_path.join("package.json"), package_json_content)?;
    fs::write(project_path.join("tsconfig.json"), tsconfig_content)?;

    Ok(())
}

fn create_js_tests(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let tests_dir = project_path.join("tests");
    fs::create_dir_all(tests_dir.join("fixtures"))?;

    if config.language == "javascript" || config.language == "typescript" {
        fs::write(tests_dir.join("fixtures").join("input.js"), JS_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.js"),
            JS_TEST_EXPECTED,
        )?;
    } else if config.language == "python" {
        fs::write(
            tests_dir.join("fixtures").join("input.py"),
            PYTHON_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.py"),
            PYTHON_TEST_EXPECTED,
        )?;
    } else if config.language == "rust" {
        fs::write(tests_dir.join("fixtures").join("input.rs"), RUST_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.rs"),
            RUST_TEST_EXPECTED,
        )?;
    } else if config.language == "go" {
        fs::write(tests_dir.join("fixtures").join("input.go"), GO_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.go"),
            GO_TEST_EXPECTED,
        )?;
    } else if config.language == "java" {
        fs::write(
            tests_dir.join("fixtures").join("input.java"),
            JAVA_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.java"),
            JAVA_TEST_EXPECTED,
        )?;
    } else if config.language == "csharp" {
        fs::write(
            tests_dir.join("fixtures").join("input.cs"),
            CSHARP_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.cs"),
            CSHARP_TEST_EXPECTED,
        )?;
    } else if config.language == "cpp" {
        fs::write(tests_dir.join("fixtures").join("input.cpp"), CPP_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.cpp"),
            CPP_TEST_EXPECTED,
        )?;
    } else if config.language == "c" {
        fs::write(tests_dir.join("fixtures").join("input.c"), C_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.c"),
            C_TEST_EXPECTED,
        )?;
    } else if config.language == "php" {
        fs::write(tests_dir.join("fixtures").join("input.php"), PHP_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.php"),
            PHP_TEST_EXPECTED,
        )?;
    } else if config.language == "ruby" {
        fs::write(tests_dir.join("fixtures").join("input.rb"), RUBY_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.rb"),
            RUBY_TEST_EXPECTED,
        )?;
    } else if config.language == "elixir" {
        fs::write(
            tests_dir.join("fixtures").join("input.ex"),
            ELIXIR_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.ex"),
            ELIXIR_TEST_EXPECTED,
        )?;
    } else if config.language == "html" {
        fs::write(
            tests_dir.join("fixtures").join("input.html"),
            HTML_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.html"),
            HTML_TEST_EXPECTED,
        )?;
    } else if config.language == "css" {
        fs::write(tests_dir.join("fixtures").join("input.css"), CSS_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.css"),
            CSS_TEST_EXPECTED,
        )?;
    } else if config.language == "kotlin" {
        fs::write(
            tests_dir.join("fixtures").join("input.kt"),
            KOTLIN_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.kt"),
            KOTLIN_TEST_EXPECTED,
        )?;
    } else if config.language == "angular" {
        fs::write(
            tests_dir.join("fixtures").join("input-angular.ts"),
            ANGULAR_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected-angular.ts"),
            ANGULAR_TEST_EXPECTED,
        )?;
    } else if config.language == "json" {
        fs::write(
            tests_dir.join("fixtures").join("input.json"),
            JSON_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.json"),
            JSON_TEST_EXPECTED,
        )?;
    } else if config.language == "yaml" {
        fs::write(
            tests_dir.join("fixtures").join("input.yaml"),
            YAML_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.yaml"),
            YAML_TEST_EXPECTED,
        )?;
    }

    Ok(())
}

fn create_gitignore(project_path: &Path) -> Result<()> {
    fs::write(project_path.join(".gitignore"), GITIGNORE_TEMPLATE)?;
    Ok(())
}

fn create_readme(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let test_command = match config.project_type {
        ProjectType::Shell => "bash scripts/transform.sh",
        ProjectType::AstGrepJs => "npm test",
        ProjectType::AstGrepYaml => "ast-grep test rules/",
        ProjectType::Hybrid => "npm test",
    };

    let readme_content = README_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{language}", &config.language)
        .replace("{test_command}", test_command)
        .replace("{license}", &config.license);

    fs::write(project_path.join("README.md"), readme_content)?;
    Ok(())
}

fn run_post_init_commands(
    project_path: &Path,
    config: &ProjectConfig,
    no_interactive: bool,
) -> Result<()> {
    match config.project_type {
        ProjectType::AstGrepJs | ProjectType::Hybrid => {
            let package_manager = if no_interactive {
                config.package_manager.clone().unwrap_or("npm".to_string())
            } else {
                Select::new(
                    "Which package manager would you like to use?",
                    vec!["npm", "yarn", "pnpm"],
                )
                .prompt()?
                .to_string()
            };

            let output = ProcessCommand::new(package_manager)
                .arg("install")
                .current_dir(project_path)
                .output();

            println!("{} Installing dependencies...", style("⏳").yellow());

            match output {
                Ok(result) => {
                    if result.status.success() {
                        println!("{CHECKMARK} Dependencies installed successfully");
                    } else {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        println!(
                            "{} Failed to install dependencies: {}",
                            style("⚠").red(),
                            stderr
                        );
                        println!(
                            "  You can run {} manually later",
                            style("npm install").cyan()
                        );
                    }
                }
                Err(e) => {
                    println!("{} npm not found: {}", style("⚠").red(), e);
                    println!(
                        "  You can run {} manually later",
                        style("npm install").cyan()
                    );
                }
            }

            // For hybrid projects, also make shell scripts executable
            if config.project_type == ProjectType::Hybrid {
                println!("{} Making scripts executable...", style("⏳").yellow());

                let scripts_dir = project_path.join("scripts");
                if let Ok(entries) = fs::read_dir(&scripts_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("sh") {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                if let Ok(mut perms) = fs::metadata(&path).map(|m| m.permissions())
                                {
                                    perms.set_mode(0o755);
                                    if fs::set_permissions(&path, perms).is_ok() {
                                        println!(
                                            "{} Made {} executable",
                                            CHECKMARK,
                                            path.file_name().unwrap().to_string_lossy()
                                        );
                                    }
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                println!(
                                    "{} {} (executable permission not set on non-Unix systems)",
                                    CHECKMARK,
                                    path.file_name().unwrap().to_string_lossy()
                                );
                            }
                        }
                    }
                }
            }
        }
        ProjectType::Shell => {
            println!("{} Making scripts executable...", style("⏳").yellow());

            let scripts_dir = project_path.join("scripts");
            if let Ok(entries) = fs::read_dir(&scripts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("sh") {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(mut perms) = fs::metadata(&path).map(|m| m.permissions()) {
                                perms.set_mode(0o755);
                                if fs::set_permissions(&path, perms).is_ok() {
                                    println!(
                                        "{} Made {} executable",
                                        CHECKMARK,
                                        path.file_name().unwrap().to_string_lossy()
                                    );
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            println!(
                                "{} {} (executable permission not set on non-Unix systems)",
                                CHECKMARK,
                                path.file_name().unwrap().to_string_lossy()
                            );
                        }
                    }
                }
            }
        }
        ProjectType::AstGrepYaml => {
            // No post-init commands needed for YAML projects
        }
    }

    Ok(())
}

fn print_next_steps(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    println!();
    println!(
        "{} Created {} project",
        CHECKMARK,
        style(&config.name).green().bold()
    );
    println!("{CHECKMARK} Generated codemod.yaml manifest");
    println!("{CHECKMARK} Generated workflow.yaml definition");
    println!("{CHECKMARK} Created project structure");
    println!();
    println!("{}", style("Next steps:").bold());

    println!();
    println!("  {}", style("Validate your workflow").bold().cyan());
    println!(
        "  {}",
        style(format!(
            "npx codemod@latest workflow validate -w {}/workflow.yaml",
            project_path.display()
        ))
        .dim()
    );
    println!();
    println!("  {}", style("Run your codemod locally").bold().cyan());
    println!(
        "  {}",
        style("Warning: Target path is where you are and please run it on git tracked path")
            .yellow()
            .bold()
    );
    println!(
        "  {}",
        style(format!(
            "npx codemod@latest workflow run -w {}/workflow.yaml --target ./some/target/path",
            project_path.display()
        ))
        .dim()
    );
    println!();
    println!(
        "  {}",
        style("👉 Check out the docs to learn how to publish your codemod!")
            .bold()
            .cyan()
    );
    println!(
        "  {}",
        style("https://go.codemod.com/docs").underlined().dim()
    );

    Ok(())
}
