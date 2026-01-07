use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Write,
    path::Path,
};

use butterflow_models::{StateDiff, Task, TaskDiff, Workflow, WorkflowRun, WorkflowRunDiff};
use clap::{Parser, Subcommand};
use serde_json::to_writer_pretty;
use ts_export::export_recursive;

mod cli_create;
mod tree_sitter_compress;
mod ts_export;

#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a JSON schema for the workflow object.
    Schema,
    /// Generate a TypeScript definition for the workflow object.
    Ts,
    /// Compress tree-sitter grammar node types to AI-friendly format.
    CompressTreeSitter,
    /// Create a new codemod project using codemod init.
    CliCreate {
        /// Target directory for the codemod project (defaults to current directory)
        #[arg(value_name = "DIR")]
        target_dir: Option<std::path::PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    match args.command {
        Commands::Schema => {
            let schema = schemars::schema_for!(Workflow);
            let Ok(xtask_path) = std::env::var("CARGO_MANIFEST_DIR") else {
                panic!("Cannot find CARGO_MANIFEST_DIR");
            };
            let schema_out_dir = Path::new(&xtask_path).parent().unwrap().join("schemas");

            let json_schema_path = schema_out_dir.join("workflow.json");
            let Ok(mut json_file) = File::create(&json_schema_path) else {
                panic!("Cannot create file: {json_schema_path:?}");
            };
            to_writer_pretty(&mut json_file, &schema).unwrap();
        }
        Commands::Ts => {
            let Ok(xtask_path) = std::env::var("CARGO_MANIFEST_DIR") else {
                panic!("Cannot find CARGO_MANIFEST_DIR");
            };
            let schema_out_dir = Path::new(&xtask_path)
                .parent()
                .unwrap()
                .join("crates/scheduler/npm");

            let ts_schema_path = schema_out_dir.join("types.ts");
            let mut ts_file = File::create(&ts_schema_path).unwrap();
            let mut type_hash_map = HashMap::new();
            let mut type_names = HashSet::new();
            export_recursive::<Workflow>(&mut type_hash_map, &mut type_names);
            export_recursive::<Task>(&mut type_hash_map, &mut type_names);
            export_recursive::<WorkflowRun>(&mut type_hash_map, &mut type_names);
            export_recursive::<TaskDiff>(&mut type_hash_map, &mut type_names);
            export_recursive::<StateDiff>(&mut type_hash_map, &mut type_names);
            export_recursive::<WorkflowRunDiff>(&mut type_hash_map, &mut type_names);
            let mut type_hash_map = type_hash_map.into_iter().collect::<Vec<_>>();
            type_hash_map.sort_by_key(|(k, _)| *k);
            for (_, ts_def) in type_hash_map {
                ts_file.write_all(ts_def.as_bytes()).unwrap();
                ts_file.write_all(b"\n").unwrap();
            }
        }
        Commands::CompressTreeSitter => {
            if let Err(e) = tree_sitter_compress::compress_tree_sitter_grammar().await {
                eprintln!("Error compressing tree-sitter grammar: {e}");
                std::process::exit(1);
            }
        }
        Commands::CliCreate { target_dir } => {
            if let Err(e) = cli_create::create_codemod(target_dir).await {
                eprintln!("Error creating codemod: {e}");
                std::process::exit(1);
            }
        }
    }
}
