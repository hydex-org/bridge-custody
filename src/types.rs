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

/// Orchard key shards derived from FROST secret shares
/// Each node holds these secrets and uses them for threshold signing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchardKeyShards {
    /// Our share of the Orchard spend authorizing key (SECRET)
    #[serde(with = "serde_bytes")]
    pub ask_share: Vec<u8>,  // 32 bytes, serialized scalar
    
    /// Our share of the Orchard nullifier deriving key (SECRET)
    #[serde(with = "serde_bytes")]
    pub nsk_share: Vec<u8>,  // 32 bytes, serialized scalar
    
    /// Shared rivk for IVK derivation (public, same for all nodes)
    #[serde(with = "serde_bytes")]
    pub rivk: Vec<u8>,       // 32 bytes, serialized scalar
    
    /// Node ID that owns this shard
    pub node_id: u16,
}

/// Contribution to the Full Viewing Key computation
/// These are PUBLIC key points that can be safely shared
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FvkContribution {
    /// Contributor's node ID
    pub node_id: u16,
    
    /// Public key point derived from ask_share: ak = [ask]B
    #[serde(with = "serde_bytes")]
    pub ak_bytes: Vec<u8>,   // 32 bytes, compressed point
    
    /// Public key point derived from nsk_share: nk = [nsk]B
    #[serde(with = "serde_bytes")]
    pub nk_bytes: Vec<u8>,   // 32 bytes, compressed point
}

/// Result of DKG ceremony including both FROST and Orchard keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DkgResult {
    pub node_id: u16,
    
    /// FROST key share (serialized KeyPackage)
    #[serde(with = "serde_bytes")]
    pub key_share: Vec<u8>,
    
    /// FROST group verifying key (public)
    #[serde(with = "serde_bytes")]
    pub group_verifying_key: Vec<u8>,
    
    /// Orchard key shards (SECRET - store securely!)
    pub orchard_shards: OrchardKeyShards,
    
    /// Bridge's Unified Address
    pub bridge_ua: String,
    
    /// Bridge's Unified Full Viewing Key (for enclave)
    pub full_viewing_key: String,
    
    /// Aggregated ak point (for child derivation) - ADD THIS
    #[serde(with = "serde_bytes")]
    pub aggregated_ak: Vec<u8>,
    
    /// Aggregated nk point (for child derivation) - ADD THIS
    #[serde(with = "serde_bytes")]
    pub aggregated_nk: Vec<u8>,
    
    /// Shared rivk (for child derivation) - ADD THIS
    #[serde(with = "serde_bytes")]
    pub shared_rivk: Vec<u8>,
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