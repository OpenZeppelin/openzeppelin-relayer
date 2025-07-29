//! # OpenAPI Specification Generator
//!
//! This utility generates an OpenAPI specification JSON file from the
//! OpenZeppelin Relayer API definitions. It doesn't require starting the full server
//! and can be used as part of documentation or CI/CD workflows.
//!
//! ## Usage
//!
//! Run the utility with optional output path parameter:
//!
//! ```bash
//!
//! # By default `openapi.json` will be in ./docs
//! cargo generate_openapi
//! ```
//!
//! ## Features
//!
//! - Generates a complete OpenAPI specification from code annotations
//! - Includes all API endpoints including Utopia network endpoints
//! - Creates output directories automatically if they don't exist
//! - Pretty-prints the JSON for better readability
//!
//! ## Integration
//!
//! This utility is commonly used in CI/CD pipelines to generate up-to-date API documentation
//! whenever the API changes. The generated file can be committed to the repository
//! or published to API documentation platforms.
use std::env;
use std::fs;
use std::path::Path;

use openzeppelin_relayer::openapi::ApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let output_path = args.get(1).map(|s| s.as_str()).unwrap_or("openapi.json");

    if let Some(parent) = Path::new(output_path).parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    println!("Generating OpenAPI specification to {}", output_path);

    let openapi = ApiDoc::openapi();

    let json = serde_json::to_string_pretty(&openapi)?;

    fs::write(output_path, json)?;

    println!("OpenAPI specification successfully generated!");

    Ok(())
}
