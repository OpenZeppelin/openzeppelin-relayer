use std::path::Path;
use std::process::Command;

fn main() {
    let plugins_dir = Path::new("plugins");
    if !plugins_dir.exists() {
        println!("cargo:warning=plugins directory not found, skipping plugin setup");
        return;
    }

    let node_modules = plugins_dir.join("node_modules");
    let sandbox_executor_ts = plugins_dir.join("lib/sandbox-executor.ts");
    let sandbox_executor_js = plugins_dir.join("lib/sandbox-executor.js");

    // Tell Cargo when to rerun this script
    println!("cargo:rerun-if-changed=plugins/lib/sandbox-executor.ts");
    println!("cargo:rerun-if-changed=plugins/package.json");

    // Check if pnpm is available
    let pnpm_check = Command::new("pnpm").arg("--version").output();
    if pnpm_check.is_err() {
        println!("cargo:warning=pnpm not found in PATH, skipping plugin setup");
        println!("cargo:warning=Run 'pnpm install && pnpm run build' in plugins/ manually");
        return;
    }

    // Only run pnpm install if node_modules is missing
    if !node_modules.exists() {
        println!("cargo:warning=Installing plugin dependencies...");
        let output = Command::new("pnpm")
            .arg("install")
            .arg("--ignore-scripts") // Skip postinstall, we'll build explicitly below
            .current_dir(plugins_dir)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                println!("cargo:warning=✓ pnpm install completed");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("cargo:warning=pnpm install failed: {}", stderr);
                return;
            }
            Err(e) => {
                println!("cargo:warning=Failed to execute pnpm install: {}", e);
                return;
            }
        }
    }

    // Build sandbox-executor if source is newer than output (or output missing)
    let needs_build = if !sandbox_executor_js.exists() {
        true
    } else {
        // Compare modification times
        match (
            sandbox_executor_ts.metadata().and_then(|m| m.modified()),
            sandbox_executor_js.metadata().and_then(|m| m.modified()),
        ) {
            (Ok(src_time), Ok(out_time)) => src_time > out_time,
            _ => true, // Rebuild if we can't determine
        }
    };

    if needs_build {
        println!("cargo:warning=Building sandbox-executor...");
        let output = Command::new("pnpm")
            .arg("run")
            .arg("build")
            .current_dir(plugins_dir)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                println!("cargo:warning=✓ sandbox-executor built successfully");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("cargo:warning=sandbox-executor build failed: {}", stderr);
            }
            Err(e) => {
                println!("cargo:warning=Failed to build sandbox-executor: {}", e);
            }
        }
    }
}
