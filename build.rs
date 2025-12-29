use std::process::Command;
use std::path::Path;

fn main() {
    // Only run if plugins directory exists
    let plugins_dir = Path::new("plugins");
    if !plugins_dir.exists() {
        println!("cargo:warning=plugins directory not found, skipping pnpm install");
        return;
    }

    // Check if pnpm is available
    let pnpm_check = Command::new("pnpm")
        .arg("--version")
        .output();

    if pnpm_check.is_err() {
        println!("cargo:warning=pnpm not found in PATH, skipping plugin dependencies install");
        println!("cargo:warning=Please install pnpm and run 'pnpm install' in the plugins directory manually");
        return;
    }

    println!("cargo:warning=Running pnpm install in plugins directory...");
    
    // Run pnpm install in plugins directory
    let output = Command::new("pnpm")
        .arg("install")
        .current_dir(plugins_dir)
        .output();

    match output {
        Ok(output) => {
            if output.status.success() {
                println!("cargo:warning=✓ pnpm install completed successfully");
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("cargo:warning=pnpm install failed: {}", stderr);
                // Don't fail the build, just warn
            }
        }
        Err(e) => {
            println!("cargo:warning=Failed to execute pnpm install: {}", e);
            // Don't fail the build, just warn
        }
    }
}

