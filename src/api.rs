use anyhow::Result;
use axum::{
    extract::{Json, State},
    routing::{get, post},  // ← ADD `get` HERE
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::frost_signer::FrostSigningCoordinator;
use crate::tx_builder::OrchardTxBuilder;
use crate::zcash_client::ZcashRpcClient;

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

/// API state shared across handlers
pub struct ApiState {
    frost_coordinator: Arc<Mutex<FrostSigningCoordinator>>,
    tx_builder: Arc<OrchardTxBuilder>,
    zcash_client: Arc<ZcashRpcClient>,
}

/// Create the withdrawal API router
pub fn create_api_router(
    frost_coordinator: FrostSigningCoordinator,
    tx_builder: OrchardTxBuilder,
    zcash_client: ZcashRpcClient,
) -> Router {
    let state = Arc::new(ApiState {
        frost_coordinator: Arc::new(Mutex::new(frost_coordinator)),
        tx_builder: Arc::new(tx_builder),
        zcash_client: Arc::new(zcash_client),
    });

    Router::new()
        .route("/api/bridge-address", get(get_bridge_address))  // ← ADD THIS LINE
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

/// Get bridge deposit address
async fn get_bridge_address(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<BridgeAddressResponse>, String> {
    let network = "testnet";  // TODO: Make configurable
    
    let ua = state.tx_builder
        .get_bridge_address(network)
        .map_err(|e| format!("Failed to get bridge address: {}", e))?;
    
    let ufvk = state.tx_builder
        .get_bridge_ufvk(network)
        .map_err(|e| format!("Failed to get UFVK: {}", e))?;
    
    Ok(Json(BridgeAddressResponse {
        unified_address: ua,
        ufvk,
        network: network.to_string(),
    }))
}