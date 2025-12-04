use anyhow::Result;
use bridge_custody::{dkg_coordinator, network_http, types};
use bridge_custody::attestation_service::{AttestationService, AttestationServiceConfig};
use bridge_custody::withdrawal_service::{WithdrawalService, WithdrawalServiceConfig};
use clap::Parser;
use std::collections::HashMap;
use std::time::Duration;
use bridge_custody::enclave_client::EnclaveClient;

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
    
    println!("Starting MPC Node {}", config.node_id);
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
            eprintln!("HTTP server error: {}", e);
        }
    });

    // Wait for all servers to start (Docker networking overhead)
    println!("Waiting for all nodes to start HTTP servers...");
    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;
    println!("Ready to begin DKG");

    // Build peer map
    let peers: HashMap<u16, String> = config
        .peers
        .iter()
        .map(|p| (p.node_id, p.address.clone()))
        .collect();

    // Create HTTP client for multi-node DKG coordination
    let http_client = network_http::HttpNetworkClient::new(node_id, peers);

    // Check if DKG result already exists
    let dkg_result_path = format!("/data/node{}_dkg_result.json", node_id);
    let _result = if std::path::Path::new(&dkg_result_path).exists() {
        // Load existing DKG result
        println!("Loading existing DKG result from {}", dkg_result_path);
        let contents = std::fs::read_to_string(&dkg_result_path)?;
        let result: types::DkgResult = serde_json::from_str(&contents)?;
        println!("Loaded existing key!");
        println!("   Bridge UA: {}", result.bridge_ua);
        println!("   UFVK: {}", result.full_viewing_key);
        result
    } else {
        // Run DKG ceremony for the first time
        println!("No existing key found. Running DKG ceremony...");
        let mut coordinator = dkg_coordinator::DkgCoordinator::new(
            node_id,
            config.total_nodes,
            config.threshold,
            http_client,
        );

        match coordinator.run_ceremony(&config.zcash.network).await {
            Ok(result) => {
                println!("DKG Complete!");
                println!("   Bridge UA: {}", result.bridge_ua);
                println!("   Group Key: {}", hex::encode(&result.group_verifying_key));
                println!("   UFVK: {}", result.full_viewing_key);
                
                // Save to disk for future restarts
                std::fs::create_dir_all("/data")?;
                std::fs::write(
                    &dkg_result_path,
                    serde_json::to_string_pretty(&result)?
                )?;
                println!("   Result saved to {}", dkg_result_path);
                result
            }
            Err(e) => {
                eprintln!("DKG failed: {}", e);
                return Err(e);
            }
        }
    };

    // =========================================================================
    // PROVISION ENCLAVE WITH UFVK
    // =========================================================================
    if config.enclave.enabled {
        println!("Provisioning enclave with UFVK...");
        let enclave_client = EnclaveClient::new(&config.enclave.url);
        
        match enclave_client.provision(&_result.full_viewing_key, &_result.bridge_ua).await {
            Ok(resp) => {
                println!("   Enclave provisioned!");
                println!("   Enclave pubkey: {}", resp.enclave_pubkey);
            }
            Err(e) => {
                // Non-fatal - enclave might already be provisioned
                println!("   Enclave provision note: {}", e);
            }
        }
    }

    // =========================================================================
    // START ATTESTATION SERVICE
    // =========================================================================
    if config.enclave.enabled {
        println!("Starting attestation service...");
        println!("   Enclave URL: {}", config.enclave.url);
        println!("   Solana RPC: {}", config.solana.rpc_url);
        println!("   Program ID: {}", config.solana.bridge_program_id);
        println!("   Keypair: {}", config.solana.keypair_path);
        println!("   Poll interval: {}s", config.enclave.poll_interval_secs);

        let attestation_config = AttestationServiceConfig {
            poll_interval: Duration::from_secs(config.enclave.poll_interval_secs),
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
        };

        let enclave_url = config.enclave.url.clone();
        let solana_rpc_url = config.solana.rpc_url.clone();
        let program_id = config.solana.bridge_program_id.clone();
        let keypair_path = config.solana.keypair_path.clone();

        // Run attestation service in background
        // Flow: Enclave -> MPC Node -> Solana (direct, no Arcium)
        tokio::spawn(async move {
            match AttestationService::new(
                &enclave_url,
                &solana_rpc_url,
                &program_id,
                &keypair_path,
                attestation_config,
            ) {
                Ok(service) => {
                    println!("Attestation service initialized");
                    if let Err(e) = service.run().await {
                        eprintln!("Attestation service error: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to create attestation service: {}", e);
                }
            }
        });

        println!("Attestation service started in background");
    } else {
        println!("Attestation service disabled (set enclave.enabled = true to enable)");
    }

    // =========================================================================
    // START WITHDRAWAL SERVICE
    // =========================================================================
    if config.withdrawal.enabled {
        println!("Starting withdrawal service...");
        println!("   Zcash RPC: {}", config.zcash.rpc_url);
        println!("   Solana RPC: {}", config.solana.rpc_url);
        println!("   Poll interval: {}s", config.withdrawal.poll_interval_secs);

        let withdrawal_config = WithdrawalServiceConfig {
            poll_interval: Duration::from_secs(config.withdrawal.poll_interval_secs),
            max_retries: 3,
            retry_delay: Duration::from_secs(10),
            min_zcash_confirmations: config.withdrawal.min_zcash_confirmations,
        };

        let zcash_rpc_url = config.zcash.rpc_url.clone();
        let zcash_user = config.zcash.rpc_user.clone();
        let zcash_pass = config.zcash.rpc_password.clone();
        let solana_rpc_url = config.solana.rpc_url.clone();
        let program_id = config.solana.bridge_program_id.clone();
        let keypair_path = config.solana.keypair_path.clone();
        let hydex_api_url = config.bridge_api.url.clone();
        let hydex_api_key = config.bridge_api.api_key.clone();
        let dkg_result_clone = _result.clone();

        tokio::spawn(async move {
            match WithdrawalService::new(
                &solana_rpc_url,
                &program_id,
                &keypair_path,
                &zcash_rpc_url,
                &zcash_user,
                &zcash_pass,
                &hydex_api_url,
                &hydex_api_key,
                &dkg_result_clone,
                withdrawal_config,
            ) {
                Ok(service) => {
                    println!("Withdrawal service initialized");
                    if let Err(e) = service.run().await {
                        eprintln!("Withdrawal service error: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to create withdrawal service: {}", e);
                }
            }
        });

        println!("Withdrawal service started in background");
    } else {
        println!("Withdrawal service disabled (set withdrawal.enabled = true to enable)");
    }

    // Start REST API server
    println!("Starting REST API server on :3000...");
    start_api_server(node_id, config.clone()).await?;

    Ok(())
}

async fn start_api_server(node_id: u16, config: types::NodeConfig) -> Result<()> {
    use axum::{
        Router,
        routing::get,
        extract::Json,
    };
    use bridge_custody::types::DkgResult;
    use serde::Serialize;
    use std::sync::Arc;
    
    // =========================================================================
    // MPC NODE API
    // 
    // Per Hydex spec, MPC nodes should ONLY handle:
    // - Bridge info (public)
    // - Withdrawal signing (FROST)
    // 
    // Address generation is handled by the ENCLAVE (TEE), not here.
    // =========================================================================
    
    // API Response Types
    #[derive(Serialize)]
    struct BridgeInfoResponse {
        unified_address: String,
        network: String,
        enclave_url: String,
        info: String,
    }
    
    #[derive(Serialize)]
    struct NodeStatusResponse {
        node_id: u16,
        status: String,
        enclave_url: String,
        note: String,
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
    let enclave_url = config.enclave.url.clone();
    
    // Handler: Get bridge info (public)
    // NOTE: UFVK is NOT exposed here - it's only in the enclave
    let dkg_for_bridge = dkg_result.clone();
    let enclave_for_bridge = enclave_url.clone();
    let get_bridge_info = move || async move {
        let network = if dkg_for_bridge.bridge_ua.starts_with("utest") {
            "testnet"
        } else {
            "mainnet"
        };
        
        Json(BridgeInfoResponse {
            unified_address: dkg_for_bridge.bridge_ua.clone(),
            network: network.to_string(),
            enclave_url: enclave_for_bridge.clone(),
            info: "Master bridge address. For deposit addresses, use the enclave API at /v1/deposit-intents".to_string(),
        })
    };
    
    // Handler: Node status
    let enclave_for_status = enclave_url.clone();
    let get_node_status = move || async move {
        Json(NodeStatusResponse {
            node_id,
            status: "running".to_string(),
            enclave_url: enclave_for_status.clone(),
            note: "MPC node for FROST threshold signing. Address generation is handled by the enclave.".to_string(),
        })
    };
    
    // Build router - MPC nodes only expose bridge info and status
    // Address generation endpoints are on the ENCLAVE, not here
    let app = Router::new()
        .route("/api/bridge-info", get(get_bridge_info))
        .route("/api/status", get(get_node_status));
    
    // Start server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("MPC API server listening on :3000");
    println!("Endpoints:");
    println!("   GET  /api/bridge-info  - Get master bridge UA (public)");
    println!("   GET  /api/status       - Node status");
    println!("");
    println!("NOTE: Address generation is handled by the ENCLAVE:");
    println!("   POST {}/v1/deposit-intents   - Create deposit intent", enclave_url);
    println!("   POST {}/v1/generate-address  - Generate deposit address", enclave_url);
    
    axum::serve(listener, app).await?;
    
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    // Tests commented out - using Docker Compose for multi-node testing
}