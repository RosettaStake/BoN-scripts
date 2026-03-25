//! Integration tests for all four transfer commands.
//!
//! Run with:
//!   cargo test --test integration_test -- --ignored --nocapture
//!
//! Verification: compare total nonce sum across sender wallets before and after.
//! Expected delta = number of transactions sent.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Config {
    network:        String,
    wallets_dir:    String,
    amount:         String,
    batch_size:     String,
    tx_per_wallet:  String,
    random_relayer: bool,
}

impl Config {
    fn from_env() -> Self {
        Self {
            network:        std::env::var("NETWORK").unwrap_or_else(|_| "http://57.129.86.55:8079".into()),
            wallets_dir:    std::env::var("WALLETS_DIR").unwrap_or_else(|_| "./wallets".into()),
            amount:         std::env::var("AMOUNT").unwrap_or_else(|_| "1".into()),
            batch_size:     std::env::var("BATCH_SIZE").unwrap_or_else(|_| "99".into()),
            tx_per_wallet:  std::env::var("TX_PER_WALLET").unwrap_or_else(|_| "100".into()),
            random_relayer: std::env::var("RANDOM_RELAYER")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }
}

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const NONCE_TIMEOUT: Duration = Duration::from_secs(120);

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn get_nonce(client: &reqwest::blocking::Client, addr: &str, network: &str) -> u64 {
    let url = format!("{}/address/{}", network, addr);
    client.get(&url).send().ok()
        .and_then(|r| r.json::<serde_json::Value>().ok())
        .and_then(|b| b.pointer("/data/account/nonce").and_then(|v| v.as_u64()))
        .unwrap_or(0)
}

fn discover_wallets_by_shard(cfg: &Config) -> HashMap<u8, Vec<String>> {
    let output = Command::new("./target/debug/sprinter")
        .args(["check-wallets", "--wallets-dir", &cfg.wallets_dir, "--network", &cfg.network])
        .stdin(Stdio::null())
        .output()
        .expect("Failed to run check-wallets");

    let mut result: HashMap<u8, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let t = line.trim();
        if !t.starts_with("erd1") { continue; }
        let parts: Vec<&str> = t.splitn(5, '|').collect();
        if parts.len() < 2 { continue; }
        let addr = parts[0].trim().to_string();
        if addr.len() < 60 { continue; }
        if let Ok(s) = parts[1].trim().parse::<u8>() {
            result.entry(s).or_default().push(addr);
        }
    }
    result
}

fn total_nonces(client: &reqwest::blocking::Client, wallets: &[String], network: &str) -> u64 {
    wallets.iter().map(|a| get_nonce(client, a, network)).sum()
}

fn wait_nonce_delta(
    client: &reqwest::blocking::Client,
    wallets: &[String],
    nonces_before: u64,
    expected_delta: u64,
    network: &str,
) {
    let deadline = Instant::now() + NONCE_TIMEOUT;
    loop {
        let current = total_nonces(client, wallets, network);
        let delta = current.saturating_sub(nonces_before);
        println!("  nonce delta: {}/{}", delta, expected_delta);
        if delta >= expected_delta {
            println!("  ✅ nonce delta reached");
            return;
        }
        assert!(Instant::now() < deadline, "Timeout: nonce delta {} < expected {}", delta, expected_delta);
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn run_sprinter(args: &[&str]) {
    let status = Command::new("./target/debug/sprinter")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to spawn sprinter. Run `cargo build` first.");
    assert!(status.success(), "sprinter exited with {}", status);
}

fn base_args<'a>(cfg: &'a Config, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "--wallets-dir", &cfg.wallets_dir,
        "--network",     &cfg.network,
        "--amount",      &cfg.amount,
        "--total-txs-per-wallet", &cfg.tx_per_wallet,
        "--batch-size",  &cfg.batch_size,
        "--no-tui",
    ];
    args.extend_from_slice(extra);
    if cfg.random_relayer { args.push("--random-relayer"); }
    args
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn test_transfer_intrashard() {
    let cfg = Config::from_env();
    let client = http_client();
    let wallets = discover_wallets_by_shard(&cfg);
    let shard0 = wallets.get(&0).expect("No shard-0 wallets");
    let expected = shard0.len() as u64 * cfg.tx_per_wallet.parse::<u64>().unwrap();

    let before = total_nonces(&client, shard0, &cfg.network);
    println!("=== test_transfer_intrashard: expect {} txs ===", expected);

    let mut args = base_args(&cfg, &["--shard", "0"]);
    args.insert(0, "transfer-intrashard");
    run_sprinter(&args);

    wait_nonce_delta(&client, shard0, before, expected, &cfg.network);
    println!("✅ PASSED");
}

#[test]
#[ignore]
fn test_transfer_cross_shard() {
    let cfg = Config::from_env();
    let client = http_client();
    let wallets = discover_wallets_by_shard(&cfg);
    let shard0 = wallets.get(&0).expect("No shard-0 wallets");
    let expected = shard0.len() as u64 * cfg.tx_per_wallet.parse::<u64>().unwrap();

    let before = total_nonces(&client, shard0, &cfg.network);
    println!("=== test_transfer_cross_shard: expect {} txs ===", expected);

    let mut args = base_args(&cfg, &["--source-shard", "0", "--destination-shard", "1"]);
    args.insert(0, "transfer-cross-shard");
    run_sprinter(&args);

    wait_nonce_delta(&client, shard0, before, expected, &cfg.network);
    println!("✅ PASSED");
}

#[test]
#[ignore]
fn test_transfer_all_shards() {
    let cfg = Config::from_env();
    let client = http_client();
    let wallets = discover_wallets_by_shard(&cfg);
    let tx_per = cfg.tx_per_wallet.parse::<u64>().unwrap();

    let all_senders: Vec<String> = [0u8, 1, 2]
        .iter()
        .flat_map(|s| wallets.get(s).into_iter().flatten().cloned())
        .collect();
    let expected = all_senders.len() as u64 * tx_per;

    let before = total_nonces(&client, &all_senders, &cfg.network);
    println!("=== test_transfer_all_shards: expect {} txs ===", expected);

    let mut args = base_args(&cfg, &[]);
    args.insert(0, "transfer-all-shards");
    run_sprinter(&args);

    wait_nonce_delta(&client, &all_senders, before, expected, &cfg.network);
    println!("✅ PASSED");
}

#[test]
#[ignore]
fn test_transfer_all_cross_shards() {
    let cfg = Config::from_env();
    let client = http_client();
    let wallets = discover_wallets_by_shard(&cfg);
    let tx_per = cfg.tx_per_wallet.parse::<u64>().unwrap();

    // Each shard sends tx_per_wallet / 6 pairs * 2 directions = tx_per_wallet total per wallet
    let all_senders: Vec<String> = [0u8, 1, 2]
        .iter()
        .flat_map(|s| wallets.get(s).into_iter().flatten().cloned())
        .collect();
    let expected = all_senders.len() as u64 * tx_per;

    let before = total_nonces(&client, &all_senders, &cfg.network);
    println!("=== test_transfer_all_cross_shards: expect {} txs ===", expected);

    let mut args = base_args(&cfg, &[]);
    args.insert(0, "transfer-all-cross-shards");
    run_sprinter(&args);

    wait_nonce_delta(&client, &all_senders, before, expected, &cfg.network);
    println!("✅ PASSED");
}
