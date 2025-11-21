use reqwest::header::CONTENT_LENGTH;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

pub type ProgressCallback = Arc<Box<dyn Fn(u64, u64) + Send + Sync>>;

pub fn get_system_info() -> (String, String, String) {
    let os: &'static str = if env::consts::OS == "macos" {
        "darwin"
    } else if env::consts::OS == "windows" {
        "win32"
    } else if env::consts::OS == "linux" {
        "linux"
    } else {
        env::consts::OS
    };
    let arch = if env::consts::ARCH == "aarch64" {
        "arm64"
    } else if env::consts::ARCH == "x86_64" {
        "x64"
    } else {
        env::consts::ARCH
    };
    let extension = if os == "darwin" {
        "dylib"
    } else if os == "linux" {
        "so"
    } else if os == "win32" {
        "dll"
    } else {
        "so"
    };
    (os.to_string(), arch.to_string(), extension.to_string())
}

pub async fn download_file(
    url: &str,
    lib_path: &PathBuf,
    progress_callback: Option<ProgressCallback>,
) -> Result<(), String> {
    if let Some(parent) = lib_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let head_response = client
        .head(url)
        .send()
        .await
        .map_err(|e| format!("Failed to get header from {url}: {e}"))?;

    let total_size = head_response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|val| val.to_str().ok()?.parse().ok())
        .unwrap_or(0);

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to download from {url}: {e}"))?;

    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(lib_path)
        .await
        .map_err(|e| format!("Failed to create file: {e}"))?;

    let mut downloaded = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error from {url}: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Write error: {e}"))?;
        downloaded += chunk.len() as u64;
        if let Some(ref callback) = progress_callback {
            callback(downloaded, total_size);
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("Flush error: {e}"))?;

    Ok(())
}
