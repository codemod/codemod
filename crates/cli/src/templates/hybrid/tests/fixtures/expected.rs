// println! macros will be matched by pattern!
// click diff tab to see rewrite and import addition.

fn main() {
    log::info!("Starting application");

    let user_id = 42;
    log::info!("User ID: {}", user_id);

    if let Some(config) = load_config() {
        log::info!("Config loaded: {:?}", config);
    } else {
        log::info!("Failed to load config");
    }

    // This is a comment with println!("not matched")
    let debug_msg = "println!(\"also not matched\")";

    match process_data() {
        Ok(result) => {
            log::info!("Processing completed successfully: {:?}", result);
        }
        Err(e) => {
            log::info!("Error processing data: {}", e);
        }
    }
}

fn load_config() -> Option<String> {
    Some("config".to_string())
}

fn process_data() -> Result<String, String> {
    Ok("data".to_string())
}
