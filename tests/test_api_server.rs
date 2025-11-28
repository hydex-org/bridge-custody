/// Simple test API server
/// Run with: cargo run --bin test_api_server

use anyhow::Result;
use bridge_custody::{types::DkgResult, address_manager::AddressManager};
use axum::{
    Router,
    routing::{get, post},
    extract::Json,
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 Starting Test API Server...\n");
    
    // Load DKG result
    let node_id = 1;
    let result_path = format!("data/node{}/node{}_dkg_result.json", node_id, node_id);
    let contents = std::fs::read_to_string(&result_path)?;
    let dkg_result: DkgResult = serde_json::from_str(&contents)?;
    let dkg_result = Arc::new(dkg_result);
    
    println!("✅ Loaded DKG result:");
    println!("   Bridge UA: {}", dkg_result.bridge_ua);
    println!("   UFVK: {}\n", dkg_result.full_viewing_key);
    
    // Initialize AddressManager
    let address_manager = Arc::new(
        AddressManager::from_ufvk(&dkg_result.full_viewing_key)
            .expect("Failed to initialize AddressManager")
    );
    
    // Handler: Get bridge address
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
    
    // Handler: Generate deposit address
    let manager_for_deposit = address_manager.clone();
    let dkg_for_deposit = dkg_result.clone();
    let generate_deposit_address = move |Json(payload): Json<DepositAddressRequest>| async move {
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
    
    // Build router
    let app = Router::new()
        .route("/api/bridge-address", get(get_bridge_address))
        .route("/api/deposit-address", post(generate_deposit_address));
    
    // Start server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await?;
    println!("✅ API server listening on :3001");
    println!("📍 Endpoints:");
    println!("   GET  /api/bridge-address     - Get master bridge info");
    println!("   POST /api/deposit-address    - Generate user deposit address\n");
    println!("🔥 Ready to accept requests!\n");
    
    axum::serve(listener, app).await?;
    
    Ok(())
}

