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
    #[serde(default)]
    pub enclave: EnclaveConfig,
    #[serde(default)]
    pub bridge_api: BridgeApiConfig,
    #[serde(default)]
    pub withdrawal: WithdrawalConfig,
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
    #[serde(default = "default_keypair_path")]
    pub keypair_path: String,
}

fn default_keypair_path() -> String {
    "/data/solana-keypair.json".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub listen_address: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnclaveConfig {
    #[serde(default = "default_enclave_url")]
    pub url: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub enabled: bool,
}

fn default_enclave_url() -> String {
    "http://localhost:8081".to_string()
}

fn default_poll_interval() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BridgeApiConfig {
    #[serde(default = "default_bridge_api_url")]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
}

fn default_bridge_api_url() -> String {
    "http://localhost:3001".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WithdrawalConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_withdrawal_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_min_zcash_confirmations")]
    pub min_zcash_confirmations: u32,
}

fn default_withdrawal_poll_interval() -> u64 {
    15
}

fn default_min_zcash_confirmations() -> u32 {
    2
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
    
    /// Aggregated ak point (for child derivation)
    #[serde(with = "serde_bytes")]
    pub aggregated_ak: Vec<u8>,
    
    /// Aggregated nk point (for child derivation)
    #[serde(with = "serde_bytes")]
    pub aggregated_nk: Vec<u8>,
    
    /// Shared rivk (for child derivation)
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

// ============================================================================
// ATTESTATION TYPES (for Enclave <-> Solana flow)
// ============================================================================

/// Attestation received from enclave (matches enclave's AttestationResponse)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnclaveAttestation {
    pub note_commitment: String,      // hex-encoded 32 bytes
    pub amount: u64,                  // zatoshis
    pub recipient_solana: String,     // hex-encoded 32 bytes (Solana pubkey)
    pub block_height: u64,
    pub enclave_signature: String,    // hex-encoded 64 bytes
    pub enclave_pubkey: String,       // hex-encoded 32 bytes
}

impl EnclaveAttestation {
    /// Parse note_commitment from hex to bytes
    pub fn note_commitment_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.note_commitment)?;
        if bytes.len() != 32 {
            anyhow::bail!("note_commitment must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }

    /// Parse recipient_solana from hex to bytes
    pub fn recipient_solana_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.recipient_solana)?;
        if bytes.len() != 32 {
            anyhow::bail!("recipient_solana must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }

    /// Parse enclave_signature from hex to bytes
    pub fn enclave_signature_bytes(&self) -> Result<[u8; 64]> {
        let bytes = hex::decode(&self.enclave_signature)?;
        if bytes.len() != 64 {
            anyhow::bail!("enclave_signature must be 64 bytes");
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }

    /// Parse enclave_pubkey from hex to bytes
    pub fn enclave_pubkey_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.enclave_pubkey)?;
        if bytes.len() != 32 {
            anyhow::bail!("enclave_pubkey must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }

    /// Serialize to the 176-byte format expected by Arcium
    /// Order: note_commitment(32) + amount(8) + recipient(32) + block_height(8) + sig(64) + pubkey(32)
    pub fn to_attestation_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(176);
        bytes.extend_from_slice(&self.note_commitment_bytes()?);
        bytes.extend_from_slice(&self.amount.to_le_bytes());
        bytes.extend_from_slice(&self.recipient_solana_bytes()?);
        bytes.extend_from_slice(&self.block_height.to_le_bytes());
        bytes.extend_from_slice(&self.enclave_signature_bytes()?);
        bytes.extend_from_slice(&self.enclave_pubkey_bytes()?);
        Ok(bytes)
    }
}

/// Status of an attestation submission
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AttestationStatus {
    Pending,
    Submitted,
    Confirmed,
    Failed(String),
}

/// Tracks the state of an attestation through the submission pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationRecord {
    pub attestation: EnclaveAttestation,
    pub deposit_id: u64,
    pub status: AttestationStatus,
    pub solana_signature: Option<String>,
    pub submitted_at: Option<i64>,
    pub confirmed_at: Option<i64>,
    pub error: Option<String>,
}