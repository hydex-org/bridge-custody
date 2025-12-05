//! Withdrawal Service
//!
//! Polls hydex-api for pending burn intents and processes withdrawals.
//! 
//! Flow: hydex-api (pending burns) -> MPC Node -> Zcash (z_sendmany) -> Solana (finalize)
//!
//! Note: Currently uses z_sendmany for simplicity. Full FROST signing with
//! OrchardTxBuilder will be implemented in Phase 1.4.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::interval;
use solana_sdk::pubkey::Pubkey;
use reqwest::Client;
use serde::Deserialize;

use crate::solana_client::BridgeSolanaClient;
use crate::zcash_client::ZcashRpcClient;
use crate::types::DkgResult;

/// Configuration for the withdrawal service
#[derive(Debug, Clone)]
pub struct WithdrawalServiceConfig {
    pub poll_interval: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
    /// Minimum confirmations before considering Zcash TX complete
    pub min_zcash_confirmations: u32,
}

impl Default for WithdrawalServiceConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(15),
            max_retries: 3,
            retry_delay: Duration::from_secs(10),
            min_zcash_confirmations: 2,
        }
    }
}

/// Burn intent status from Solana
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BurnStatus {
    Pending = 0,
    Processing = 1,
    Completed = 2,
    Failed = 3,
}

impl From<u8> for BurnStatus {
    fn from(val: u8) -> Self {
        match val {
            0 => BurnStatus::Pending,
            1 => BurnStatus::Processing,
            2 => BurnStatus::Completed,
            3 => BurnStatus::Failed,
            _ => BurnStatus::Failed,
        }
    }
}

/// Burn intent data from Solana
#[derive(Debug, Clone)]
pub struct BurnIntent {
    pub burn_id: u64,
    pub user: Pubkey,
    pub amount: u64,
    pub status: BurnStatus,
    pub encrypted_data_hash: [u8; 32],
    pub zcash_txid: [u8; 32],
}

/// Withdrawal record for tracking
#[derive(Debug, Clone)]
pub struct WithdrawalRecord {
    pub burn_id: u64,
    pub user: Pubkey,
    pub amount: u64,
    pub zcash_operation_id: Option<String>,
    pub zcash_txid: Option<String>,
    pub status: WithdrawalStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WithdrawalStatus {
    Pending,
    ZcashSubmitted,
    ZcashConfirmed,
    SolanaFinalized,
    Failed(String),
}

/// Response from hydex-api pending burns endpoint
#[derive(Debug, Deserialize)]
struct PendingBurnsResponse {
    count: usize,
    items: Vec<PendingBurnItem>,
}

#[derive(Debug, Deserialize)]
struct PendingBurnItem {
    burn_id: u64,
    user: String,
    amount: String,
    zcash_address_hash: String,
    status: String,
    network: String,
}

/// Response from hydex-api burn address endpoint
#[derive(Debug, Deserialize)]
struct BurnAddressResponse {
    burn_id: u64,
    user: String,
    amount: String,
    zcash_address: String,
    status: String,
    network: String,
}

/// Service that processes withdrawals from Solana burn intents
pub struct WithdrawalService {
    solana_client: Arc<BridgeSolanaClient>,
    zcash_client: Arc<ZcashRpcClient>,
    http_client: Client,
    config: WithdrawalServiceConfig,
    /// Bridge unified address (source for withdrawals)
    bridge_ua: String,
    /// Hydex API URL
    hydex_api_url: String,
    /// Hydex API key
    hydex_api_key: String,
    /// Track in-flight withdrawals
    pending_withdrawals: Arc<Mutex<Vec<WithdrawalRecord>>>,
}

impl WithdrawalService {
    /// Create a new withdrawal service
    pub fn new(
        solana_rpc_url: &str,
        program_id: &str,
        solana_keypair_path: &str,
        zcash_rpc_url: &str,
        zcash_user: &str,
        zcash_pass: &str,
        hydex_api_url: &str,
        hydex_api_key: &str,
        dkg_result: &DkgResult,
        config: WithdrawalServiceConfig,
    ) -> Result<Self> {
        let solana_client = Arc::new(
            BridgeSolanaClient::new(solana_rpc_url, program_id, solana_keypair_path)?
        );
        
        let zcash_client = Arc::new(
            ZcashRpcClient::new(
                zcash_rpc_url.to_string(),
                zcash_user.to_string(),
                zcash_pass.to_string(),
            )?
        );

        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            solana_client,
            zcash_client,
            http_client,
            config,
            bridge_ua: dkg_result.bridge_ua.clone(),
            hydex_api_url: hydex_api_url.to_string(),
            hydex_api_key: hydex_api_key.to_string(),
            pending_withdrawals: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Run the withdrawal service (main loop)
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Starting withdrawal service");
        tracing::info!("   Poll interval: {:?}", self.config.poll_interval);
        tracing::info!("   Bridge UA: {}", &self.bridge_ua[..20]);

        // Verify connections
        self.verify_connections().await?;

        let mut poll_timer = interval(self.config.poll_interval);

        loop {
            poll_timer.tick().await;

            // Process pending burn intents
            if let Err(e) = self.process_pending_burns().await {
                tracing::error!("Error processing burns: {}", e);
            }

            // Check status of in-flight Zcash operations
            if let Err(e) = self.check_pending_operations().await {
                tracing::error!("Error checking operations: {}", e);
            }
        }
    }

    /// Verify Solana and Zcash connections
    async fn verify_connections(&self) -> Result<()> {
        // Check Solana
        tracing::info!("Checking Solana connection...");
        match self.solana_client.check_balance().await {
            Ok(balance) => {
                tracing::info!("   Solana: OK (payer balance: {} lamports)", balance);
            }
            Err(e) => {
                tracing::error!("   Solana: unreachable - {}", e);
                anyhow::bail!("Cannot reach Solana: {}", e);
            }
        }

        // Check Zcash
        tracing::info!("Checking Zcash connection...");
        match self.zcash_client.get_blockchain_info().await {
            Ok(info) => {
                tracing::info!("   Zcash: OK (chain: {}, blocks: {})", info.chain, info.blocks);
            }
            Err(e) => {
                tracing::error!("   Zcash: unreachable - {}", e);
                anyhow::bail!("Cannot reach Zcash: {}", e);
            }
        }

        // Check bridge balance
        match self.zcash_client.z_getbalance(&self.bridge_ua, Some(1)).await {
            Ok(balance) => {
                tracing::info!("   Bridge balance: {} ZEC", balance);
            }
            Err(e) => {
                tracing::warn!("   Could not fetch bridge balance: {}", e);
            }
        }

        Ok(())
    }

    /// Process pending burn intents from Solana
    async fn process_pending_burns(&self) -> Result<()> {
        // Fetch pending burn intents
        let burns = self.fetch_pending_burns().await?;

        if burns.is_empty() {
            tracing::debug!("No pending burn intents");
            return Ok(());
        }

        tracing::info!("Found {} pending burn intents", burns.len());

        for burn in burns {
            if let Err(e) = self.process_single_burn(&burn).await {
                tracing::error!(
                    "Failed to process burn #{}: {}",
                    burn.burn_id,
                    e
                );
            }
        }

        Ok(())
    }

    /// Fetch pending burn intents from hydex-api
    async fn fetch_pending_burns(&self) -> Result<Vec<BurnIntent>> {
        let url = format!("{}/v1/internal/burns/pending", self.hydex_api_url);
        
        let response = self.http_client
            .get(&url)
            .header("X-API-Key", &self.hydex_api_key)
            .send()
            .await
            .context("Failed to fetch pending burns from hydex-api")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("hydex-api error {}: {}", status, body);
        }

        let pending: PendingBurnsResponse = response.json().await
            .context("Failed to parse pending burns response")?;

        let burns: Vec<BurnIntent> = pending.items.into_iter().map(|item| {
            let user = item.user.parse::<Pubkey>().unwrap_or_default();
            let mut encrypted_data_hash = [0u8; 32];
            if let Ok(hash_bytes) = hex::decode(&item.zcash_address_hash) {
                let len = std::cmp::min(hash_bytes.len(), 32);
                encrypted_data_hash[..len].copy_from_slice(&hash_bytes[..len]);
            }
            
            BurnIntent {
                burn_id: item.burn_id,
                user,
                amount: item.amount.parse().unwrap_or(0),
                status: match item.status.as_str() {
                    "Pending" => BurnStatus::Pending,
                    "Processing" => BurnStatus::Processing,
                    _ => BurnStatus::Pending,
                },
                encrypted_data_hash,
                zcash_txid: [0; 32],
            }
        }).collect();

        Ok(burns)
    }

    /// Process a single burn intent
    async fn process_single_burn(&self, burn: &BurnIntent) -> Result<()> {
        tracing::info!(
            "Processing burn #{}: {} zatoshi from {}",
            burn.burn_id,
            burn.amount,
            burn.user
        );

        // Mark as processing on Solana
        self.mark_burn_processing(burn).await?;

        // Get the Zcash destination address
        // Note: In production, you'd decrypt this from Arcium or get it from hydex-api
        let destination_address = self.resolve_zcash_address(burn).await?;

        // Convert zatoshi to ZEC (1 ZEC = 100_000_000 zatoshi)
        let amount_zec = burn.amount as f64 / 100_000_000.0;

        // Send via z_sendmany (simplified flow - no FROST signing yet)
        tracing::info!(
            "Sending {} ZEC to {}",
            amount_zec,
            &destination_address[..20]
        );

        let operation_id = self.zcash_client
            .z_sendmany(
                &self.bridge_ua,
                vec![(&destination_address, amount_zec)],
                1,  // min confirmations
                None,  // default fee
            )
            .await
            .context("Failed to initiate Zcash transfer")?;

        tracing::info!("Zcash operation initiated: {}", operation_id);

        // Track the withdrawal
        let record = WithdrawalRecord {
            burn_id: burn.burn_id,
            user: burn.user,
            amount: burn.amount,
            zcash_operation_id: Some(operation_id),
            zcash_txid: None,
            status: WithdrawalStatus::ZcashSubmitted,
            error: None,
        };

        self.pending_withdrawals.lock().await.push(record);

        Ok(())
    }

    /// Mark burn intent as processing on Solana
    async fn mark_burn_processing(&self, burn: &BurnIntent) -> Result<()> {
        match self.solana_client.mark_burn_processing(burn.burn_id, &burn.user).await {
            Ok(sig) => {
                tracing::info!("Marked burn #{} as processing: {}", burn.burn_id, sig);
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to mark burn #{} as processing: {}", burn.burn_id, e);
                Err(e)
            }
        }
    }

    /// Resolve Zcash address from burn intent via hydex-api
    async fn resolve_zcash_address(&self, burn: &BurnIntent) -> Result<String> {
        let url = format!(
            "{}/v1/internal/burns/{}/address",
            self.hydex_api_url,
            burn.burn_id
        );
        
        let response = self.http_client
            .get(&url)
            .header("X-API-Key", &self.hydex_api_key)
            .send()
            .await
            .context("Failed to fetch Zcash address from hydex-api")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to get Zcash address for burn #{}: {} - {}",
                burn.burn_id,
                status,
                body
            );
        }

        let addr_response: BurnAddressResponse = response.json().await
            .context("Failed to parse address response")?;

        tracing::info!(
            "Resolved Zcash address for burn #{}: {}...",
            burn.burn_id,
            &addr_response.zcash_address[..20]
        );

        Ok(addr_response.zcash_address)
    }

    /// Check status of pending Zcash operations
    async fn check_pending_operations(&self) -> Result<()> {
        let mut withdrawals = self.pending_withdrawals.lock().await;
        
        for withdrawal in withdrawals.iter_mut() {
            if !matches!(withdrawal.status, WithdrawalStatus::ZcashSubmitted) {
                continue;
            }

            let Some(ref op_id) = withdrawal.zcash_operation_id else {
                continue;
            };

            // Check operation status
            match self.zcash_client.z_getoperationstatus(Some(vec![op_id.clone()])).await {
                Ok(status) => {
                    if let Some(result) = status.as_array().and_then(|a| a.first()) {
                        let op_status = result.get("status").and_then(|s| s.as_str());
                        
                        match op_status {
                            Some("success") => {
                                if let Some(txid) = result.get("result").and_then(|r| r.get("txid")).and_then(|t| t.as_str()) {
                                    tracing::info!(
                                        "Withdrawal #{} Zcash TX confirmed: {}",
                                        withdrawal.burn_id,
                                        txid
                                    );
                                    withdrawal.zcash_txid = Some(txid.to_string());
                                    withdrawal.status = WithdrawalStatus::ZcashConfirmed;
                                    
                                    // Finalize on Solana
                                    if let Err(e) = self.finalize_withdrawal(withdrawal).await {
                                        tracing::error!(
                                            "Failed to finalize withdrawal #{} on Solana: {}",
                                            withdrawal.burn_id,
                                            e
                                        );
                                    }
                                }
                            }
                            Some("failed") => {
                                let error = result.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str());
                                let error_msg = error.unwrap_or("Unknown error").to_string();
                                tracing::error!(
                                    "Withdrawal #{} Zcash TX failed: {}",
                                    withdrawal.burn_id,
                                    error_msg
                                );
                                withdrawal.status = WithdrawalStatus::Failed(error_msg.clone());
                                withdrawal.error = Some(error_msg);
                                
                                // Mark as failed on Solana
                                if let Err(e) = self.finalize_withdrawal_failed(withdrawal).await {
                                    tracing::error!(
                                        "Failed to mark withdrawal #{} as failed on Solana: {}",
                                        withdrawal.burn_id,
                                        e
                                    );
                                }
                            }
                            Some("executing") | Some("queued") => {
                                tracing::debug!(
                                    "Withdrawal #{} still processing...",
                                    withdrawal.burn_id
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to check operation status for withdrawal #{}: {}",
                        withdrawal.burn_id,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Finalize successful withdrawal on Solana
    async fn finalize_withdrawal(&self, withdrawal: &mut WithdrawalRecord) -> Result<()> {
        let Some(ref txid) = withdrawal.zcash_txid else {
            anyhow::bail!("No Zcash txid available");
        };

        // Convert txid to bytes (pad/truncate to 32 bytes)
        let txid_bytes = hex::decode(txid).unwrap_or_default();
        let mut zcash_txid = [0u8; 32];
        let len = std::cmp::min(txid_bytes.len(), 32);
        zcash_txid[..len].copy_from_slice(&txid_bytes[..len]);

        match self.solana_client.finalize_withdrawal(
            withdrawal.burn_id,
            &withdrawal.user,
            zcash_txid,
            true,  // success
        ).await {
            Ok(sig) => {
                tracing::info!(
                    "Finalized withdrawal #{} on Solana: {} (zcash txid: {})",
                    withdrawal.burn_id,
                    sig,
                    txid
                );
                withdrawal.status = WithdrawalStatus::SolanaFinalized;
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to finalize withdrawal #{}: {}", withdrawal.burn_id, e);
                Err(e)
            }
        }
    }

    /// Mark withdrawal as failed on Solana
    async fn finalize_withdrawal_failed(&self, withdrawal: &mut WithdrawalRecord) -> Result<()> {
        match self.solana_client.finalize_withdrawal(
            withdrawal.burn_id,
            &withdrawal.user,
            [0u8; 32],
            false,  // failed
        ).await {
            Ok(sig) => {
                tracing::info!(
                    "Marked withdrawal #{} as failed on Solana: {}",
                    withdrawal.burn_id,
                    sig
                );
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to mark withdrawal #{} as failed: {}", withdrawal.burn_id, e);
                Err(e)
            }
        }
    }

    /// Get withdrawal statistics
    pub async fn get_stats(&self) -> WithdrawalStats {
        let withdrawals = self.pending_withdrawals.lock().await;
        
        let pending = withdrawals.iter()
            .filter(|w| matches!(w.status, WithdrawalStatus::Pending))
            .count();
        let zcash_submitted = withdrawals.iter()
            .filter(|w| matches!(w.status, WithdrawalStatus::ZcashSubmitted))
            .count();
        let completed = withdrawals.iter()
            .filter(|w| matches!(w.status, WithdrawalStatus::SolanaFinalized))
            .count();
        let failed = withdrawals.iter()
            .filter(|w| matches!(w.status, WithdrawalStatus::Failed(_)))
            .count();

        WithdrawalStats {
            total_processed: withdrawals.len(),
            pending,
            zcash_submitted,
            completed,
            failed,
        }
    }
}

/// Statistics about withdrawal processing
#[derive(Debug, Clone)]
pub struct WithdrawalStats {
    pub total_processed: usize,
    pub pending: usize,
    pub zcash_submitted: usize,
    pub completed: usize,
    pub failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burn_status_conversion() {
        assert_eq!(BurnStatus::from(0), BurnStatus::Pending);
        assert_eq!(BurnStatus::from(1), BurnStatus::Processing);
        assert_eq!(BurnStatus::from(2), BurnStatus::Completed);
        assert_eq!(BurnStatus::from(3), BurnStatus::Failed);
        assert_eq!(BurnStatus::from(255), BurnStatus::Failed);
    }

    #[test]
    fn test_config_default() {
        let config = WithdrawalServiceConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(15));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.min_zcash_confirmations, 2);
    }
}

