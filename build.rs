use std::path::Path;
use std::process::Command;

fn main() {
    let plugins_dir = Path::new("plugins");
    if !plugins_dir.exists() {
        println!("cargo:warning=plugins directory not found, skipping plugin setup");
        return;
    }

    let node_modules = plugins_dir.join("node_modules");
    let esbuild_bin = node_modules.join(".bin/esbuild");
    let pool_executor_ts = plugins_dir.join("lib/pool-executor.ts");
    let pool_executor_js = plugins_dir.join("lib/pool-executor.js");

    // Tell Cargo when to rerun this script
    println!("cargo:rerun-if-changed=plugins/lib/pool-executor.ts");
    println!("cargo:rerun-if-changed=plugins/package.json");

    // Check if pnpm is available
    let pnpm_check = Command::new("pnpm").arg("--version").output();
    if pnpm_check.is_err() {
        println!("cargo:warning=pnpm not found in PATH, skipping plugin setup");
        println!("cargo:warning=Run 'pnpm install && pnpm run build' in plugins/ manually");
        return;
    }

    // Check if node_modules or esbuild is missing - need to install dependencies
    let needs_install = !node_modules.exists() || !esbuild_bin.exists();

    if needs_install {
        if node_modules.exists() && !esbuild_bin.exists() {
            println!(
                "cargo:warning=esbuild not found in node_modules, reinstalling dependencies..."
            );
        } else {
            println!("Installing plugin dependencies...");
        }

        let output = Command::new("pnpm")
            .arg("install")
            .arg("--ignore-scripts") // Skip postinstall, we'll build explicitly below
            .current_dir(plugins_dir)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                println!("✓ pnpm install completed");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("cargo:warning=pnpm install failed: {stderr}");
                return;
            }
            Err(e) => {
                println!("cargo:warning=Failed to execute pnpm install: {e}");
                return;
            }
        }

        // Verify esbuild is now available after install
        if !esbuild_bin.exists() {
            println!("cargo:warning=esbuild still not found after pnpm install");
            println!("cargo:warning=Try running 'pnpm install' manually in plugins/ directory");
            return;
        }
    }

    // Build pool-executor if source is newer than output (or output missing)
    let needs_build = if !pool_executor_js.exists() {
        true
    } else {
        // Compare modification times
        match (
            pool_executor_ts.metadata().and_then(|m| m.modified()),
            pool_executor_js.metadata().and_then(|m| m.modified()),
        ) {
            (Ok(src_time), Ok(out_time)) => src_time > out_time,
            _ => true, // Rebuild if we can't determine
        }
    };

    if needs_build {
        println!("Building executor...");
        let output = Command::new("pnpm")
            .arg("run")
            .arg("build")
            .current_dir(plugins_dir)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                println!("✓ executor built successfully");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("cargo:warning=executor build failed: {stderr}");
            }
            Err(e) => {
                println!("cargo:warning=Failed to build executor: {e}");
            }
        }
    }
}
