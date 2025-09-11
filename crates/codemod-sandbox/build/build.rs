fn main() {
    // Set environment variables to help with binaryen-sys CMake build
    println!("cargo:rustc-env=CMAKE_BUILD_PARALLEL_LEVEL=1");

    // Ensure CMake uses a clean build directory
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let cmake_build_dir = std::path::Path::new(&out_dir).join("cmake_build");
        if cmake_build_dir.exists() {
            let _ = std::fs::remove_dir_all(&cmake_build_dir);
        }
    }

    // Force CMake to use a specific generator to avoid conflicts
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-env=CMAKE_GENERATOR=Unix Makefiles");
    }
}
