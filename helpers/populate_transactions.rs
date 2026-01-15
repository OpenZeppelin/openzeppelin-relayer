//! Helper script to populate large number of test transactions to Redis for performance testing
//!
//! Usage: cargo run --manifest-path=Cargo.toml --example populate_transactions -- \
//!   --redis-url "redis://127.0.0.1:6379" \
//!   --key-prefix "test" \
//!   --relayer-id "relayer-1" \
//!   --transaction-count 4000 \
//!   --batch-size 100

use chrono::{Duration, Utc};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde_json::json;
use std::time::Instant;
use uuid::Uuid;

const RELAYER_PREFIX: &str = "relayer";
const TX_PREFIX: &str = "tx";
const STATUS_PREFIX: &str = "status";
const NONCE_PREFIX: &str = "nonce";
const TX_TO_RELAYER_PREFIX: &str = "tx_to_relayer";
const RELAYER_LIST_KEY: &str = "relayer_list";
const TX_BY_CREATED_AT_PREFIX: &str = "tx_by_created_at";

struct Config {
    redis_url: String,
    key_prefix: String,
    relayer_id: String,
    transaction_count: u32,
    batch_size: u32,
}

fn parse_args() -> Config {
    let args: Vec<String> = std::env::args().collect();
    let mut config = Config {
        redis_url: "redis://127.0.0.1:6379".to_string(),
        key_prefix: "test".to_string(),
        relayer_id: "relayer-1".to_string(),
        transaction_count: 4000,
        batch_size: 100,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--redis-url" => {
                if i + 1 < args.len() {
                    config.redis_url = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--key-prefix" => {
                if i + 1 < args.len() {
                    config.key_prefix = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--relayer-id" => {
                if i + 1 < args.len() {
                    config.relayer_id = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--transaction-count" => {
                if i + 1 < args.len() {
                    config.transaction_count = args[i + 1].parse().unwrap_or(4000);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--batch-size" => {
                if i + 1 < args.len() {
                    config.batch_size = args[i + 1].parse().unwrap_or(100);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    config
}

fn tx_key(key_prefix: &str, relayer_id: &str, tx_id: &str) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX, tx_id
    )
}

fn tx_to_relayer_key(key_prefix: &str, tx_id: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        key_prefix, RELAYER_PREFIX, TX_TO_RELAYER_PREFIX, tx_id
    )
}

fn relayer_status_key(key_prefix: &str, relayer_id: &str, status: &str) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        key_prefix, RELAYER_PREFIX, relayer_id, STATUS_PREFIX, status
    )
}

fn relayer_nonce_key(key_prefix: &str, relayer_id: &str, nonce: u64) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        key_prefix, RELAYER_PREFIX, relayer_id, NONCE_PREFIX, nonce
    )
}

fn relayer_list_key(key_prefix: &str) -> String {
    format!("{}:{}", key_prefix, RELAYER_LIST_KEY)
}

fn relayer_tx_by_created_at_key(key_prefix: &str, relayer_id: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        key_prefix, RELAYER_PREFIX, relayer_id, TX_BY_CREATED_AT_PREFIX
    )
}

fn created_at_to_score(created_at: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.timestamp_millis() as f64)
        .unwrap_or(0.0)
}

fn create_transaction_json(tx_id: &str, relayer_id: &str, nonce: u64, created_at: &str) -> String {
    // Generate ~5KB of data for realistic payload testing
    let large_data = "0x".to_string() + &"abcdef0123456789".repeat(320); // ~5KB

    json!({
        "id": tx_id,
        "relayer_id": relayer_id,
        "status": "confirmed",
        "status_reason": null,
        "created_at": created_at,
        "sent_at": null,
        "confirmed_at": null,
        "valid_until": null,
        "delete_at": created_at,
        "network_type": "evm",
        "priced_at": null,
        "hashes": [],
        "network_data": {
            "network_data": "Evm",
            "data": {
                "gas_price": null,
                "gas_limit": 21000,
                "nonce": nonce,
                "value": "0x1",
                "data": large_data,
                "from": "0xSender",
                "to": "0xRecipient",
                "chain_id": 1,
                "signature": null,
                "hash": format!("0x{}", tx_id),
                "speed": "fast",
                "max_fee_per_gas": null,
                "max_priority_fee_per_gas": null,
                "raw": null
            }
        },
        "noop_count": null,
        "is_canceled": false
    })
    .to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args();

    println!("🚀 Starting transaction population...");
    println!("   Redis URL: {}", config.redis_url);
    println!("   Key Prefix: {}", config.key_prefix);
    println!("   Relayer ID: {}", config.relayer_id);
    println!("   Transaction Count: {}", config.transaction_count);
    println!("   Batch Size: {}", config.batch_size);

    let client = Client::open(config.redis_url.as_str())?;
    let mut conn = ConnectionManager::new(client).await?;

    // Start from a base time and spread transactions across 30 days
    let base_time = Utc::now() - Duration::days(30);

    let start = Instant::now();
    let mut total_created = 0u32;

    for batch_num in 0..((config.transaction_count + config.batch_size - 1) / config.batch_size) {
        let mut pipe = redis::pipe();
        pipe.atomic();

        let batch_start = batch_num * config.batch_size;
        let batch_end = std::cmp::min(batch_start + config.batch_size, config.transaction_count);

        for i in batch_start..batch_end {
            let tx_id = Uuid::new_v4().to_string();
            let nonce = i as u64;

            // Spread created_at across 30 days
            let days_offset = (i % 30) as i64;
            let seconds_offset = if config.transaction_count > 30 {
                ((i / 30) * 86400 / (config.transaction_count / 30)) as i64
            } else {
                ((i as i64 * 86400) / std::cmp::max(1, config.transaction_count as i64)) as i64
            };
            let created_at =
                (base_time + Duration::days(days_offset) + Duration::seconds(seconds_offset))
                    .to_rfc3339();

            // Transaction data
            let tx_key = tx_key(&config.key_prefix, &config.relayer_id, &tx_id);
            let tx_json = create_transaction_json(&tx_id, &config.relayer_id, nonce, &created_at);
            pipe.set(&tx_key, &tx_json);

            // Reverse lookup
            let reverse_key = tx_to_relayer_key(&config.key_prefix, &tx_id);
            pipe.set(&reverse_key, &config.relayer_id);

            // Add to relayer list
            let list_key = relayer_list_key(&config.key_prefix);
            pipe.sadd(&list_key, &config.relayer_id);

            // Status index (use capitalized form to match Display trait output)
            let status_key =
                relayer_status_key(&config.key_prefix, &config.relayer_id, "Confirmed");
            pipe.sadd(&status_key, &tx_id);

            // Nonce index
            let nonce_key = relayer_nonce_key(&config.key_prefix, &config.relayer_id, nonce);
            pipe.set(&nonce_key, &tx_id);

            // Sorted set by created_at
            let created_at_score = created_at_to_score(&created_at);
            let sorted_key = relayer_tx_by_created_at_key(&config.key_prefix, &config.relayer_id);
            pipe.zadd(&sorted_key, &tx_id, created_at_score);

            total_created += 1;
        }

        // Execute batch
        pipe.exec_async(&mut conn).await?;

        let progress = ((total_created as f64 / config.transaction_count as f64) * 100.0) as u32;
        let elapsed = start.elapsed().as_secs_f64();
        let rate = total_created as f64 / elapsed;

        println!(
            "✓ Created {:5} transactions ({:3}%) | {:.0} tx/sec | {:.1}s elapsed",
            total_created, progress, rate, elapsed
        );
    }

    let elapsed = start.elapsed().as_secs_f64();
    let rate = config.transaction_count as f64 / elapsed;

    println!("\n✅ Population complete!");
    println!("   Total transactions: {}", config.transaction_count);
    println!("   Time elapsed: {:.2}s", elapsed);
    println!("   Average rate: {:.0} tx/sec", rate);

    // Verify data was created
    println!("\n📊 Verification:");

    let relayer_list_key = relayer_list_key(&config.key_prefix);
    let relayers: u64 = conn.scard(&relayer_list_key).await?;
    println!("   Relayers in system: {}", relayers);

    let sorted_key = relayer_tx_by_created_at_key(&config.key_prefix, &config.relayer_id);
    let tx_count: u64 = conn.zcard(&sorted_key).await?;
    println!("   Transactions for relayer: {}", tx_count);

    let status_key = relayer_status_key(&config.key_prefix, &config.relayer_id, "Confirmed");
    let confirmed_count: u64 = conn.scard(&status_key).await?;
    println!("   Confirmed transactions: {}", confirmed_count);

    // Check memory usage
    let info: String = redis::cmd("INFO")
        .arg("memory")
        .query_async(&mut conn)
        .await?;

    println!("\n💾 Redis Memory Info:");
    for line in info.lines() {
        if line.contains("used_memory") || line.contains("used_memory_human") {
            println!("   {}", line);
        }
    }

    Ok(())
}
