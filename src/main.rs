//! Complete UFVK Balance Scanner with Real Blockchain Data

mod scanner;
mod grpc_client;
mod lightwalletd;

use anyhow::{Result, bail};
use clap::Parser;
use reqwest::Client;
use zcash_address::{unified, Network};
use zcash_address::unified::{Encoding, Container};
use indicatif::{ProgressBar, ProgressStyle};
use scanner::{OrchardScanner, BalanceResult};
use grpc_client::LightwalletdClient;


#[derive(serde::Serialize)]
struct EmitOrchardQuery {
    data: Vec<u8>,
    height: u64,
}

#[derive(Parser)]
#[command(name = "ufvk-scanner")]
#[command(about = "Scan Zcash blockchain for UFVK balance with REAL data", long_about = None)]
struct Cli {
    /// Unified Full Viewing Key
    #[arg(short, long)]
    ufvk: String,

    /// Lightwalletd server URL
    #[arg(short, long, default_value = "https://testnet.zec.rocks:443")]
    server: String,

    /// Network (testnet or mainnet)
    #[arg(short, long, default_value = "testnet")]
    network: String,
    
    /// Number of blocks to scan (default: 1000)
    #[arg(short, long, default_value = "1000")]
    blocks: u64,
    
    /// Bridge address for context
    #[arg(short, long)]
    address: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let cli = Cli::parse();

    println!("\n=========================================");
    println!("   Hydex UFVK Real Balance Scanner");
    println!("=========================================\n");

    // Parse network
    let network = match cli.network.as_str() {
        "testnet" | "test" => Network::Test,
        "mainnet" | "main" => Network::Main,
        _ => bail!("Invalid network"),
    };

    println!("Network:     {:?}", network);
    println!("Server:      {}", cli.server);
    println!("Scan depth:  {} blocks\n", cli.blocks);

    // Decode UFVK
    println!("=== Step 1: Decoding UFVK ===");
    let decoded = match unified::Ufvk::decode(&cli.ufvk) {
        Ok((parsed_net, ufvk)) => {
            if parsed_net != network {
                bail!("Network mismatch");
            }
            println!("✓ UFVK valid for {:?}", parsed_net);
            ufvk
        }
        Err(e) => bail!("Invalid UFVK: {:?}", e),
    };

    // Extract Orchard FVK
    let mut orchard_fvk_bytes: Option<[u8; 96]> = None;
    for item in decoded.items() {
        if let unified::Fvk::Orchard(bytes) = item {
            if bytes.len() == 96 {
                let mut arr = [0u8; 96];
                arr.copy_from_slice(&bytes);
                orchard_fvk_bytes = Some(arr);
                println!("✓ Orchard FVK extracted (96 bytes)");
                break;
            }
        }
    }

    let fvk_bytes = orchard_fvk_bytes
        .ok_or_else(|| anyhow::anyhow!("No Orchard FVK in UFVK"))?;

    // Create scanner
    let mut scanner = OrchardScanner::new(&fvk_bytes)?;
    println!("✓ Scanner initialized\n");

    // Connect to lightwalletd
    println!("=== Step 2: Connecting to Blockchain ===");
    print!("Connecting to {}... ", cli.server);
    
    let mut client = match LightwalletdClient::connect(cli.server.clone()).await {
        Ok(c) => {
            println!("✓");
            c
        }
        Err(e) => {
            println!("✗");
            println!("\n❌ Failed to connect: {}\n", e);
            println!("💡 Common issues:");
            println!("   • Server might be down");
            println!("   • Try a different server:");
            println!("     --server https://lightwalletd.testnet.electriccoin.co:9067");
            println!("   • Check your internet connection");
            return Err(e);
        }
    };
    
    let latest_height = client.get_latest_block().await?;
    println!("✓ Latest block: {}", latest_height);

    let start_height = latest_height.saturating_sub(cli.blocks);
    println!("✓ Will scan blocks {} to {}\n", start_height, latest_height);

    // Scan blocks
    println!("=== Step 3: Scanning Blockchain ===");
    let pb = ProgressBar::new(cli.blocks);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} blocks ({eta}) | {msg}")
        .unwrap()
        .progress_chars("#>-"));

    let mut result = BalanceResult::default();
    
    // Scan in batches to avoid timeouts
    let batch_size = 100u64;
    
    for batch_start in (start_height..=latest_height).step_by(batch_size as usize) {
        let batch_end = (batch_start + batch_size - 1).min(latest_height);
        
        pb.set_message(format!("Fetching blocks {}-{}", batch_start, batch_end));
        
        let link = "http://localhost:8080/zec/emit_orchard";
        let httpClient = Client::new();
        match client.get_block_range(batch_start, batch_end).await {
            Ok(blocks) => {
                pb.set_message(format!("Scanning {} blocks", blocks.len()));
                
                for block in blocks {
                    result.blocks_scanned += 1;
                    
                                    // ---- Build the JSON body the server expects ----
                    let query = EmitOrchardQuery {
                        data: block.hash.clone().to_vec(), // or however block.hash is represented
                        height: block.height,
                    };
                    let html = httpClient.post(link).json(&query).send().await?.text().await?;
                    //println!("{}",html);
                    // Scan each transaction
                    for tx in block.vtx {
                        // Scan each Orchard action
                        for action in tx.actions {
                            result.actions_scanned += 1;
                            result.decryption_attempts += 1;
                            
                            // Try to decrypt
                            if let Some(value) = scanner.try_decrypt_action(
                                &action.nullifier,
                                &action.cmx,
                                &action.ephemeral_key,
                                &action.ciphertext,
                            ) {
                                result.received_value += value;
                                result.received_count += 1;
                                pb.println(format!("  ✓ Found note: {} zatoshis", value));
                            }
                        }
                    }
                    
                    pb.inc(1);
                }
            }
            Err(e) => {
                pb.println(format!("⚠️  Error fetching blocks {}-{}: {}", batch_start, batch_end, e));
            }
        }
    }

    pb.finish_with_message("Scan complete!");

    // Display results
    println!("\n=========================================");
    println!("   Balance Report");
    println!("=========================================\n");

    println!("Blocks scanned:       {}", result.blocks_scanned);
    println!("Actions scanned:      {}", result.actions_scanned);
    println!("Decryption attempts:  {}", result.decryption_attempts);
    println!("Nullifiers tracked:   {}", scanner.nullifier_count());
    println!("\nNotes found:          {}", result.received_count);
    println!("Total received:       {} zatoshis", result.received_value);
    println!("                      {:.8} ZEC", result.received_value as f64 / 100_000_000.0);

    if let Some(addr) = &cli.address {
        println!("\nBridge address:       {}...", &addr[..50.min(addr.len())]);
    }

    if result.received_count == 0 {
        println!("\n💡 No transactions found in the last {} blocks.", cli.blocks);
        println!("   This could mean:");
        println!("   • No funds have been sent to this address recently");
        println!("   • Funds were sent more than {} blocks ago", cli.blocks);
        println!("   • Try scanning more blocks with --blocks 10000");
    }

    println!("\n=========================================\n");


    Ok(())
}