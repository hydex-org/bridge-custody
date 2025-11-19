use anyhow::Result;
use bridge_custody::{dkg_coordinator, mpc_node, network, network_http, types};
use clap::Parser;
use std::collections::HashMap;

#[derive(Parser, Debug)]
#[clap(name = "mpc-node")]
#[clap(about = "MPC custody node for Zcash-Solana bridge", long_about = None)]
struct Cli {
    /// Path to configuration file
    #[clap(short, long, default_value = "config/node.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bridge_custody=info".parse()?)
        )
        .init();

    let cli = Cli::parse();

    // Load configuration
    let config = types::NodeConfig::load(&cli.config)?;
    
    println!("🚀 Starting MPC Node {}", config.node_id);
    println!("   Network: {} nodes, threshold {}", config.total_nodes, config.threshold);
    println!("   Zcash: {}", config.zcash.network);

    run_service(config).await?;

    Ok(())
}

async fn run_service(config: types::NodeConfig) -> Result<()> {
    use network_http::{HttpNetworkServer, HttpNetworkClient};

    let node_id = config.node_id;
    let listen_addr = format!("{}:{}", config.network.listen_address, config.network.port);

    // Start HTTP server
    let server = HttpNetworkServer::new(node_id);
    let server_clone = server.clone();
    
    tokio::spawn(async move {
        if let Err(e) = server_clone.serve(listen_addr).await {
            eprintln!("❌ HTTP server error: {}", e);
        }
    });

    // Wait for all servers to start (Docker networking overhead)
    println!("⏳ Waiting for all nodes to start HTTP servers...");
    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;
    println!("✓ Ready to begin DKG");

    // Build peer map
    let peers: HashMap<u16, String> = config
        .peers
        .iter()
        .map(|p| (p.node_id, p.address.clone()))
        .collect();

    // Create HTTP client
    let client = HttpNetworkClient::new(node_id, peers);

    // Run DKG ceremony
    println!("🔐 Initializing with DKG ceremony...");
    let mut coordinator = dkg_coordinator::DkgCoordinator::new_with_http(
        node_id,
        config.total_nodes,
        config.threshold,
        client,
    );

    match coordinator.run_ceremony(&config.zcash.network).await {
        Ok(result) => {
            println!("✅ DKG Complete!");
            println!("   Bridge UA: {}", result.bridge_ua);
            println!("   Group Key: {}", hex::encode(&result.group_verifying_key));
            println!("   👁️  UFVK: {}", result.full_viewing_key);
            
            // Save to disk
            std::fs::create_dir_all("/data")?;
            std::fs::write(
                format!("/data/node{}_dkg_result.json", node_id),
                serde_json::to_string_pretty(&result)?
            )?;
            println!("   💾 Result saved to /data/node{}_dkg_result.json", node_id);
        }
        Err(e) => {
            eprintln!("❌ DKG failed: {}", e);
            return Err(e);
        }
    }

    // Keep server running
    println!("🌐 Node running... Press Ctrl+C to stop");
    
    tokio::signal::ctrl_c().await?;
    println!("👋 Shutting down");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    
    #[tokio::test]
    async fn test_three_node_dkg_ceremony() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("bridge_custody=debug,test=debug")
            .try_init();
        
        println!("\n🧪 Testing 3-Node DKG Ceremony");
        println!("================================\n");
        
        let network_storage = network::create_shared_network();
        
        let mut handles = vec![];
        
        for node_id in 1..=3 {
            let storage = Arc::clone(&network_storage);
            
            let handle = tokio::spawn(async move {
                println!("✓ Node {} starting DKG", node_id);
                
                let config = types::NodeConfig {
                    node_id,
                    total_nodes: 3,
                    threshold: 2,
                    peers: vec![],
                    zcash: types::ZcashConfig {
                        rpc_url: "http://localhost:18232".to_string(),
                        rpc_user: "test".to_string(),
                        rpc_password: "test".to_string(),
                        network: "testnet".to_string(),
                    },
                    solana: types::SolanaConfig {
                        rpc_url: "http://localhost:8899".to_string(),
                        bridge_program_id: "test".to_string(),
                    },
                    network: types::NetworkConfig {
                        listen_address: "127.0.0.1".to_string(),
                        port: 50050 + node_id,
                    },
                };
                
                let mut node = mpc_node::MpcNode::new(config.clone(), storage)
                    .expect("Failed to create node");
                
                println!("✓ Node {} initialized", node_id);
                
                // Use testnet for test
                let mut coordinator = dkg_coordinator::DkgCoordinator::new(
                    config.node_id,
                    config.total_nodes,
                    config.threshold,
                    node.network.clone(),
                );
                
                match coordinator.run_ceremony("testnet").await {
                    Ok(result) => {
                        println!("✓ Node {} DKG complete: {}", node_id, result.bridge_ua);
                        Ok(result)
                    }
                    Err(e) => {
                        println!("✗ Node {} DKG failed: {}", node_id, e);
                        Err(format!("Node {} failed: {}", node_id, e))
                    }
                }
            });
            
            handles.push(handle);
        }
        
        let mut results = vec![];
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(result)) => {
                    println!("✅ Node {} completed DKG", i + 1);
                    println!("   Bridge UA: {}\n", result.bridge_ua);
                    results.push(result);
                }
                Ok(Err(e)) => {
                    println!("❌ Node {} failed: {}", i + 1, e);
                    panic!("Node {} failed", i + 1);
                }
                Err(e) => {
                    println!("❌ Node {} panicked: {:?}", i + 1, e);
                    panic!("Node {} failed", i + 1);
                }
            }
        }
        
        assert_eq!(results.len(), 3, "All 3 nodes should complete");
        assert_eq!(results[0].bridge_ua, results[1].bridge_ua, "Node 1 and 2 should have same UA");
        assert_eq!(results[1].bridge_ua, results[2].bridge_ua, "Node 2 and 3 should have same UA");
        
        // Verify it's a real Zcash address
        assert!(results[0].bridge_ua.starts_with("utest1"), "Should be testnet UA");
        assert!(!results[0].bridge_ua.contains("PLACEHOLDER"), "Should not be placeholder");
        
        // Verify UFVK is ZIP 316-compliant
        assert!(results[0].full_viewing_key.starts_with("uviewtest"), "Should be ZIP 316 testnet UFVK");
        
        println!("✅ SUCCESS! All nodes generated the same bridge address:");
        println!("   UA: {}", results[0].bridge_ua);
        println!("   UFVK: {}", results[0].full_viewing_key);
        println!("\n🎉 DKG Ceremony Complete!");
    }
}