use anyhow::Result;
use bridge_custody::{dkg_coordinator, network_http, types};
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
    use network_http::HttpNetworkServer;

    let node_id = config.node_id;
    let listen_addr = format!("{}:{}", config.network.listen_address, config.network.port);

    // Start HTTP server for DKG coordination
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

    // Create HTTP client for multi-node DKG coordination (needed for DKG if no result exists)
    let http_client = network_http::HttpNetworkClient::new(node_id, peers);

    // Check if DKG result already exists
    let dkg_result_path = format!("/data/node{}_dkg_result.json", node_id);
    let result = if std::path::Path::new(&dkg_result_path).exists() {
        // Load existing DKG result
        println!("📂 Loading existing DKG result from {}", dkg_result_path);
        let contents = std::fs::read_to_string(&dkg_result_path)?;
        let result: types::DkgResult = serde_json::from_str(&contents)?;
        println!("✅ Loaded existing key!");
        println!("   Bridge UA: {}", result.bridge_ua);
        println!("   👁️  UFVK: {}", result.full_viewing_key);
        result
    } else {
        // Run DKG ceremony for the first time
        println!("🔐 No existing key found. Running DKG ceremony...");
        let mut coordinator = dkg_coordinator::DkgCoordinator::new(
            node_id,
            config.total_nodes,
            config.threshold,
            http_client,
        );

        match coordinator.run_ceremony(&config.zcash.network).await {
            Ok(result) => {
                println!("✅ DKG Complete!");
                println!("   Bridge UA: {}", result.bridge_ua);
                println!("   Group Key: {}", hex::encode(&result.group_verifying_key));
                println!("   👁️  UFVK: {}", result.full_viewing_key);
                
                // Save to disk for future restarts
                std::fs::create_dir_all("/data")?;
                std::fs::write(
                    &dkg_result_path,
                    serde_json::to_string_pretty(&result)?
                )?;
                println!("   💾 Result saved to {}", dkg_result_path);
                result
            }
            Err(e) => {
                eprintln!("❌ DKG failed: {}", e);
                return Err(e);
            }
        }
    };

    // Start REST API server
    println!("🌐 Starting REST API server on :3000...");
    start_api_server(node_id, config.clone()).await?;

    Ok(())
}

async fn start_api_server(node_id: u16, _config: types::NodeConfig) -> Result<()> {
    use axum::{
        Router,
        routing::{get, post},
        extract::Json,
        http::StatusCode,
    };
    use bridge_custody::{types::DkgResult, address_manager::AddressManager};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    
    // API Response Types
    #[derive(Serialize)]
    struct BridgeAddressResponse {
        unified_address: String,
        ufvk: String,
        network: String,
        info: String,
    }
    
    #[derive(Deserialize)]
    struct DepositAddressRequest {
        solana_pubkey: String,
    }
    
    #[derive(Serialize)]
    struct DepositAddressResponse {
        deposit_address: String,
        diversifier_index: u32,
        solana_pubkey: String,
        network: String,
        ufvk: String,
        note: String,
    }
    
    #[derive(Serialize)]
    struct AddressListItem {
        solana_pubkey: String,
        diversifier_index: u32,
        zcash_address: String,
    }
    
    #[derive(Serialize)]
    struct ListAddressesResponse {
        total_addresses: usize,
        addresses: Vec<AddressListItem>,
        ufvk: String,
        network: String,
    }
    
    // Load DKG result once (try both /data and data/ paths)
    let dkg_result: DkgResult = {
        let result_path_abs = format!("/data/node{}_dkg_result.json", node_id);
        let result_path_rel1 = format!("data/node{}/node{}_dkg_result.json", node_id, node_id);
        let result_path_rel2 = format!("data/node{}_dkg_result.json", node_id);
        
        let path = if std::path::Path::new(&result_path_abs).exists() {
            result_path_abs
        } else if std::path::Path::new(&result_path_rel1).exists() {
            result_path_rel1
        } else {
            result_path_rel2
        };
        
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read DKG result from {}: {}", path, e))?;
        serde_json::from_str(&contents)?
    };
    
    let dkg_result = Arc::new(dkg_result);
    
    // Initialize AddressManager with the UFVK
    let address_manager = Arc::new(
        AddressManager::from_ufvk(&dkg_result.full_viewing_key)
            .expect("Failed to initialize AddressManager")
    );
    
    // Handler: Get master bridge address and info
    let dkg_for_bridge = dkg_result.clone();
    let get_bridge_address = move || async move {
        let network = if dkg_for_bridge.bridge_ua.starts_with("utest") {
            "testnet"
        } else {
            "mainnet"
        };
        
        Json(BridgeAddressResponse {
            unified_address: dkg_for_bridge.bridge_ua.clone(),
            ufvk: dkg_for_bridge.full_viewing_key.clone(),
            network: network.to_string(),
            info: "This is the master bridge address. Use /api/deposit-address to generate user-specific deposit addresses.".to_string(),
        })
    };
    
    // Handler: Generate deposit address (NATIVE Zcash diversification)
    let manager_for_deposit = address_manager.clone();
    let dkg_for_deposit = dkg_result.clone();
    let generate_deposit_address = move |Json(payload): Json<DepositAddressRequest>| async move {
        // Validate Solana pubkey format (base58 or hex)
        if payload.solana_pubkey.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "solana_pubkey cannot be empty".to_string()));
        }
        
        // Generate deposit address using NATIVE Orchard diversification
        let (deposit_address, div_index) = manager_for_deposit
            .generate_deposit_address(&payload.solana_pubkey)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to generate address: {}", e)))?;
        
        let network = if dkg_for_deposit.bridge_ua.starts_with("utest") {
            "testnet"
        } else {
            "mainnet"
        };
        
        Ok::<_, (StatusCode, String)>(Json(DepositAddressResponse {
            deposit_address,
            diversifier_index: div_index,
            solana_pubkey: payload.solana_pubkey,
            network: network.to_string(),
            ufvk: dkg_for_deposit.full_viewing_key.clone(),
            note: "This address is derived using native Orchard diversification. The enclave can view all deposits with the single UFVK.".to_string(),
        }))
    };
    
    // Handler: List all generated addresses
    let manager_for_list = address_manager.clone();
    let dkg_for_list = dkg_result.clone();
    let list_all_addresses = move || async move {
        let mappings = manager_for_list.get_all_mappings();
        
        // Convert to a more readable format
        let addresses: Vec<AddressListItem> = mappings
            .into_iter()
            .map(|(solana_pk, (div_idx, zcash_addr))| {
                AddressListItem {
                    solana_pubkey: solana_pk,
                    diversifier_index: div_idx,
                    zcash_address: zcash_addr,
                }
            })
            .collect();
        
        let network = if dkg_for_list.bridge_ua.starts_with("utest") {
            "testnet"
        } else {
            "mainnet"
        };
        
        Json(ListAddressesResponse {
            total_addresses: addresses.len(),
            addresses,
            ufvk: dkg_for_list.full_viewing_key.clone(),
            network: network.to_string(),
        })
    };
    
    // Build router
    let app = Router::new()
        .route("/api/bridge-address", get(get_bridge_address))
        .route("/api/deposit-address", post(generate_deposit_address))
        .route("/api/list-addresses", get(list_all_addresses));
    
    // Start server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("✅ API server listening on :3000");
    println!("📍 Endpoints:");
    println!("   GET  /api/bridge-address     - Get master bridge info");
    println!("   POST /api/deposit-address    - Generate user deposit address");
    println!("   GET  /api/list-addresses     - List all generated addresses");
    
    axum::serve(listener, app).await?;
    
    Ok(())
}

// Tests are commented out - using Docker Compose for multi-node testing instead
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    /*
    use super::*;
    use bridge_custody::{mpc_node::MpcNode, network};
    
    #[tokio::test]
    async fn test_three_node_dkg_ceremony() {
        println!("🧪 Testing 3-Node DKG Ceremony");
        println!("================================");
        
        // Create shared network storage
        let network_storage = network::create_shared_network();
        
        // Spawn 3 nodes
        let mut handles = vec![];
        
        for node_id in 1..=3 {
            let storage = network_storage.clone();
            
            let handle = tokio::spawn(async move {
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
                        listen_address: "0.0.0.0".to_string(),
                        port: 8080 + node_id,
                    },
                };
                
                let mut node = MpcNode::new(config, storage).unwrap();
                println!("✓ Node {} starting DKG", node_id);
                
                let result = node.initialize_with_dkg().await.unwrap();
                println!("✓ Node {} DKG complete: {}", node_id, result);
                
                result
            });
            
            handles.push(handle);
        }
        
        // Wait for all nodes
        let mut results = vec![];
        for handle in handles {
            let result = handle.await.unwrap();
            results.push(result);
        }
        
        println!("\n✅ All nodes completed DKG");
        println!("   Node 1 Bridge UA: {}", results[0]);
        println!("   Node 2 Bridge UA: {}", results[1]);
        println!("   Node 3 Bridge UA: {}", results[2]);
        
        // Verify all nodes generated the same bridge address
        assert_eq!(results[0], results[1], "Nodes 1 and 2 should have same bridge UA");
        assert_eq!(results[1], results[2], "Nodes 2 and 3 should have same bridge UA");
        
        // Should be testnet address
        assert!(results[0].starts_with("utest1"), "Should be testnet UA");
        
        println!("\n✅ Test PASSED!");
    }
    */
}