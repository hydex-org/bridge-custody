use anyhow::Result;
use axum::{
    extract::{Json, State},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// NOTE: This file is currently not used. The API is implemented in main.rs instead.
// Keeping this for reference/future use.

use crate::frost_signer::FrostSigningCoordinator;
use crate::tx_builder::OrchardTxBuilder;
use crate::zcash_client::ZcashRpcClient;
use crate::types::DkgResult;
// use crate::orchard_frost;  // Commented out - child derivation removed
use crate::ua_builder::BridgeAddressGenerator;

/// Withdrawal request from user
#[derive(Debug, Deserialize)]
pub struct WithdrawalRequest {
    /// Recipient Zcash address
    pub to_address: String,
    /// Amount in zatoshis
    pub amount: u64,
}

/// Withdrawal response
#[derive(Debug, Serialize)]
pub struct WithdrawalResponse {
    /// Transaction ID
    pub txid: String,
    /// Status message
    pub status: String,
}

/// Bridge address response
#[derive(Debug, Serialize)]
pub struct BridgeAddressResponse {
    pub unified_address: String,
    pub ufvk: String,
    pub network: String,
}

/// Derive child address request
#[derive(Debug, Deserialize)]
pub struct DeriveAddressRequest {
    /// Solana public key (64-char hex string)
    pub solana_pubkey: String,
    /// Derivation nonce (unique per user)
    pub nonce: u64,
}

/// Derive child address response
#[derive(Debug, Serialize)]
pub struct DeriveAddressResponse {
    /// Child Unified Address (unique to this user)
    pub child_address: String,
    /// Child UFVK (for scanning this user's deposits)
    pub child_ufvk: String,
    /// Echo back the Solana pubkey
    pub solana_pubkey: String,
    /// Echo back the nonce
    pub nonce: u64,
    /// Network
    pub network: String,
}

/// API state shared across handlers
pub struct ApiState {
    frost_coordinator: Arc<Mutex<FrostSigningCoordinator>>,
    tx_builder: Arc<OrchardTxBuilder>,
    zcash_client: Arc<ZcashRpcClient>,
    node_id: u16,
}

/// Create the API router
pub fn create_api_router(
    frost_coordinator: FrostSigningCoordinator,
    tx_builder: OrchardTxBuilder,
    zcash_client: ZcashRpcClient,
    node_id: u16,
) -> Router {
    let state = Arc::new(ApiState {
        frost_coordinator: Arc::new(Mutex::new(frost_coordinator)),
        tx_builder: Arc::new(tx_builder),
        zcash_client: Arc::new(zcash_client),
        node_id,
    });

    Router::new()
        .route("/api/bridge-address", get(get_bridge_address))
        // .route("/api/derive-address", post(derive_child_address))  // Removed - use AddressManager in main.rs
        .route("/api/withdraw", post(handle_withdrawal))
        .with_state(state)
}

/// Handle withdrawal request
async fn handle_withdrawal(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<WithdrawalRequest>,
) -> Result<Json<WithdrawalResponse>, String> {
    // 1. Build transaction
    let tx_bytes = state.tx_builder
        .build_withdrawal(&req.to_address, req.amount)
        .map_err(|e| format!("Failed to build transaction: {}", e))?;
    
    // 2. TODO: Sign with FROST (coordinate between nodes)
    // let signature = frost_coordinator.sign_message(&tx_hash, &key_pkg, &pub_pkg).await?;
    
    // 3. Submit to zcashd
    let tx_hex = hex::encode(&tx_bytes);
    let txid = state.zcash_client
        .send_raw_transaction(tx_hex)
        .await
        .map_err(|e| format!("Failed to submit transaction: {}", e))?;
    
    Ok(Json(WithdrawalResponse {
        txid,
        status: "Transaction submitted".to_string(),
    }))
}

/// Get master bridge deposit address
async fn get_bridge_address(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<BridgeAddressResponse>, (StatusCode, String)> {
    let dkg_result = load_dkg_result(state.node_id)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("DKG not complete: {}", e)))?;
    
    let network = if dkg_result.bridge_ua.starts_with("utest") {
        "testnet"
    } else {
        "mainnet"
    };
    
    Ok(Json(BridgeAddressResponse {
        unified_address: dkg_result.bridge_ua,
        ufvk: dkg_result.full_viewing_key,
        network: network.to_string(),
    }))
}

/// Derive a child address for a specific Solana user
/// NOTE: This function is commented out because child key derivation was removed.
/// Use the AddressManager in main.rs for native Orchard diversification instead.
/*
async fn derive_child_address(
    State(state): State<Arc<ApiState>>,
    Json(payload): Json<DeriveAddressRequest>,
) -> Result<Json<DeriveAddressResponse>, (StatusCode, String)> {
    // Load DKG result
    let dkg_result = load_dkg_result(state.node_id)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("DKG not complete: {}", e)))?;
    
    // Parse Solana pubkey (expect hex format)
    let solana_pubkey_bytes = parse_solana_pubkey(&payload.solana_pubkey)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    
    // REMOVED: Child derivation no longer used
    // let child_ufvk = orchard_frost::derive_child_ufvk(
    //     &dkg_result.full_viewing_key,
    //     &solana_pubkey_bytes,
    //     payload.nonce,
    // ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Derivation failed: {}", e)))?;
    
    // Derive child address from child UFVK
    let network = if dkg_result.bridge_ua.starts_with("utest") {
        zcash_primitives::consensus::Network::TestNetwork
    } else {
        zcash_primitives::consensus::Network::MainNetwork
    };
    
    let child_address = BridgeAddressGenerator::generate_address_from_ufvk(
        &child_ufvk,
        network,
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Address gen failed: {}", e)))?;
    
    let network_str = if network == zcash_primitives::consensus::Network::TestNetwork {
        "testnet"
    } else {
        "mainnet"
    };
    
    Ok(Json(DeriveAddressResponse {
        child_address,
        child_ufvk,
        solana_pubkey: payload.solana_pubkey,
        nonce: payload.nonce,
        network: network_str.to_string(),
    }))
}
*/

/// Load DKG result from disk
fn load_dkg_result(node_id: u16) -> Result<DkgResult> {
    let data_path = std::env::var("DATA_PATH").unwrap_or_else(|_| "/data".to_string());
    let result_path = format!("{}/node{}_dkg_result.json", data_path, node_id);
    
    let contents = std::fs::read_to_string(&result_path)?;
    let dkg_result: DkgResult = serde_json::from_str(&contents)?;
    
    Ok(dkg_result)
}

/// Parse Solana pubkey from hex string
fn parse_solana_pubkey(s: &str) -> Result<[u8; 32], String> {
    // Try hex format
    if let Ok(bytes) = hex::decode(s) {
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            return Ok(arr);
        }
    }
    
    Err("Invalid Solana pubkey format (use 64-char hex)".to_string())
}