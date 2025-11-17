use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Deserialize)]
pub struct NodeConfig {
    pub node_id: u16,
    pub total_nodes: u16,
    pub threshold: u16,
    pub peers: Vec<PeerInfo>,
    pub zcash: ZcashConfig,
    pub solana: SolanaConfig,
    pub network: NetworkConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PeerInfo {
    pub node_id: u16,
    pub grpc_address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZcashConfig {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub network: String, // "mainnet" or "testnet"
}

#[derive(Debug, Clone, Deserialize)]
pub struct SolanaConfig {
    pub rpc_url: String,
    pub bridge_program_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    pub listen_address: String,
    pub port: u16,
}

impl NodeConfig {
    pub fn load(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .context("Failed to read config file")?;
        let config: NodeConfig = toml::from_str(&content)
            .context("Failed to parse config file")?;
        Ok(config)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalRequest {
    pub burn_log_id: String,
    pub user_solana_address: String,
    pub amount: u64,
    pub zcash_recipient: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SigningStatus {
    Initiated,
    Round1Complete,
    Round2Complete,
    Finalized { txid: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DkgResult {
    pub node_id: u16,
    #[serde(with = "serde_bytes")]
    pub key_share: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub group_verifying_key: Vec<u8>,
    pub bridge_ua: String,
}