use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node_id: u16,
    pub total_nodes: u16,
    pub threshold: u16,
    pub peers: Vec<PeerInfo>,
    pub zcash: ZcashConfig,
    pub solana: SolanaConfig,
    pub network: NetworkConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_id: u16,
    pub address: String, // e.g., "http://mpc-node2:8080"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZcashConfig {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub network: String, // "mainnet" or "testnet"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConfig {
    pub rpc_url: String,
    pub bridge_program_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub listen_address: String,
    pub port: u16,
}

impl NodeConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: NodeConfig = toml::from_str(&contents)?;
        Ok(config)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DkgResult {
    pub node_id: u16,
    pub key_share: Vec<u8>,
    pub group_verifying_key: Vec<u8>,
    pub bridge_ua: String,
    pub full_viewing_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalRequest {
    pub burn_id: String,
    pub amount: u64,
    pub recipient_address: String,
    pub solana_requester: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SigningStatus {
    Pending,
    InProgress,
    Complete,
    Failed,
}