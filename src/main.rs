mod types;
mod mpc_node;
mod dkg_coordinator;
mod frost_signer;
mod zcash_client;
mod ua_builder;
mod network;
mod solana_listener;

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Node ID (1, 2, or 3)
    #[arg(short, long)]
    node_id: u16,
    
    /// Path to configuration file
    #[arg(short, long, default_value = "config/node.toml")]
    config: String,
    
    /// Run DKG initialization
    #[arg(long)]
    init_dkg: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("bridge_custody=debug,info")
        .init();
    
    let args = Args::parse();
    
    info!("🚀 Bridge Custody MPC Node Starting");
    info!("   Node ID: {}", args.node_id);
    info!("   Config: {}", args.config);
    
    // Load configuration
    let config = types::NodeConfig::load(&args.config)?;
    
    // Create MPC node
    let mut node = mpc_node::MpcNode::new(config)?;
    
    if args.init_dkg {
        // Initialize with DKG ceremony
        info!("🔐 Starting DKG initialization...");
        let bridge_ua = node.initialize_with_dkg().await?;
        info!("✅ Bridge UA generated: {}", bridge_ua);
    } else {
        // Load existing keys
        info!("📂 Loading existing keys...");
        node.load_existing_keys()?;
    }
    
    // Start the node
    info!("🎯 Starting MPC node services...");
    node.run().await?;
    
    Ok(())
}