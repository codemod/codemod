use std::{fs::File, io::Write, path::Path};

use butterflow_models::Workflow;
use clap::{Parser, Subcommand};
use serde_json::to_writer_pretty;

mod tree_sitter_compress;

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
    /// Compress tree-sitter grammar node types to AI-friendly format.
    CompressTreeSitter,
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
            json_file.write_all(b"\n").unwrap();
        }
        Commands::CompressTreeSitter => {
            if let Err(e) = tree_sitter_compress::compress_tree_sitter_grammar().await {
                eprintln!("Error compressing tree-sitter grammar: {e}");
                std::process::exit(1);
            }
        }
    }
}
