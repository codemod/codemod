fn main() {
    let url = std::env::var("TREE_SITTER_BASE_URL")
        .unwrap_or_else(|_| "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com".to_string());
    println!("cargo:rustc-env=TREE_SITTER_BASE_URL={url}");
}
