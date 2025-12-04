//! Attestation Service
//! 
//! Continuously polls the enclave for pending attestations
//! and submits them directly to Solana using mint_simple.
//!
//! Flow: Enclave -> MPC Node -> Solana (direct, no Arcium)

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::interval;
use solana_sdk::pubkey::Pubkey;
//use std::str::FromStr;
//use rand::Rng;
use crate::enclave_client::EnclaveClient;
use crate::solana_client::BridgeSolanaClient as SolanaClient;
use crate::types::{AttestationRecord, AttestationStatus, EnclaveAttestation};

/// Configuration for the attestation service
#[derive(Debug, Clone)]
pub struct AttestationServiceConfig {
    pub poll_interval: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
}

impl Default for AttestationServiceConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(10),
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
        }
    }
}

/// Service that submits enclave attestations directly to Solana
/// 
/// Flow: Enclave -> MPC Node -> Solana (mint_simple)
pub struct AttestationService {
    enclave_client: EnclaveClient,
    solana_client: Arc<SolanaClient>,
    config: AttestationServiceConfig,
    /// Track in-flight attestations
    pending_submissions: Arc<Mutex<Vec<AttestationRecord>>>,
}

impl AttestationService {
    /// Create a new attestation service (direct Solana submission)
    pub fn new(
        enclave_url: &str,
        solana_rpc_url: &str,
        program_id: &str,
        keypair_path: &str,
        config: AttestationServiceConfig,
    ) -> Result<Self> {
        let enclave_client = EnclaveClient::new(enclave_url);
        let solana_client = Arc::new(SolanaClient::new(solana_rpc_url, program_id, keypair_path)?);

        Ok(Self {
            enclave_client,
            solana_client,
            config,
            pending_submissions: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Run the attestation service (main loop)
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Starting attestation service");
        tracing::info!("   Poll interval: {:?}", self.config.poll_interval);

        // Verify connections
        self.verify_connections().await?;

        let mut poll_timer = interval(self.config.poll_interval);

        loop {
            poll_timer.tick().await;
            
            // Add random jitter (0-5 seconds) to reduce race conditions
            let jitter = rand::random::<u64>() % 5000;
            tokio::time::sleep(std::time::Duration::from_millis(jitter)).await;
            
            if let Err(e) = self.process_pending_attestations().await {
                tracing::error!("Error processing attestations: {}", e);
            }
        }
    }

    /// Verify enclave and Solana connections
    async fn verify_connections(&self) -> Result<()> {
        // Check enclave
        tracing::info!("Checking enclave connection...");
        match self.enclave_client.health_check().await {
            Ok(true) => tracing::info!("   Enclave: OK"),
            Ok(false) => {
                tracing::warn!("   Enclave: unhealthy");
                anyhow::bail!("Enclave health check failed");
            }
            Err(e) => {
                tracing::error!("   Enclave: unreachable - {}", e);
                anyhow::bail!("Cannot reach enclave: {}", e);
            }
        }

        // Check enclave provisioning
        let status = self.enclave_client.get_status().await?;
        if !status.provisioned {
            tracing::warn!("   Enclave not provisioned - waiting for UFVK");
        } else {
            tracing::info!("   Enclave provisioned, pubkey: {}...", &status.enclave_pubkey[..16]);
        }

        // Check Solana connection
        tracing::info!("Checking Solana connection...");
        match self.solana_client.check_balance().await {            Ok(balance) => {
        tracing::info!("   Solana: OK (payer balance: {} lamports)", balance);
            }
            Err(e) => {
                tracing::error!("   Solana: unreachable - {}", e);
                anyhow::bail!("Cannot reach Solana: {}", e);
            }
        }

        Ok(())
    }

    /// Process all pending attestations
    async fn process_pending_attestations(&self) -> Result<()> {
        // Fetch pending attestations from enclave
        let attestations = match self.enclave_client.get_pending_attestations().await {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!("Failed to fetch attestations: {}", e);
                return Ok(()); // Non-fatal, retry next tick
            }
        };

        if attestations.is_empty() {
            tracing::debug!("No pending attestations");
            return Ok(());
        }

        tracing::info!("Found {} pending attestations", attestations.len());

        for attestation in attestations {
            if let Err(e) = self.process_single_attestation(&attestation).await {
                tracing::error!(
                    "Failed to process attestation (note: {}...): {}",
                    &attestation.note_commitment[..16],
                    e
                );
                // Continue with other attestations
            }
        }

        Ok(())
    }

    /// Process a single attestation - submit directly to Solana
    async fn process_single_attestation(&self, attestation: &EnclaveAttestation) -> Result<()> {
        tracing::info!(
            "Processing attestation: amount={} zatoshi, block={}, recipient={}...",
            attestation.amount,
            attestation.block_height,
            &attestation.recipient_solana[..16]
        );

        // Parse the recipient Solana address
        let pubkey_bytes = hex::decode(&attestation.recipient_solana)
        .map_err(|e| anyhow::anyhow!("Invalid hex in recipient_solana: {}", e))?;
    let user_pubkey = Pubkey::try_from(pubkey_bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("Invalid Solana pubkey bytes: {}", e))?;

        // TODO: Look up the correct deposit_id from the on-chain state
        // For now, we're using a placeholder - in production this needs proper lookup
        let deposit_id = 0u64;
        tracing::warn!("Using placeholder deposit_id={} - implement proper lookup", deposit_id);

        // Submit directly to Solana using mint_simple
        let result = self.solana_client.submit_attestation(attestation, deposit_id, &user_pubkey).await;

        match result {
            Ok(submit_result) => {
                tracing::info!(
                    "Attestation submitted to Solana: deposit_id={}, tx={}",
                    submit_result.deposit_id,
                    submit_result.signature
                );
                
                // Mark as submitted in enclave
                self.enclave_client
                    .mark_attestation_submitted(&attestation.note_commitment)
                    .await?;

                // Track submission
                let record = AttestationRecord {
                    attestation: attestation.clone(),
                    deposit_id: submit_result.deposit_id,
                    status: AttestationStatus::Submitted,
                    solana_signature: Some(submit_result.signature),
                    submitted_at: Some(chrono_timestamp()),
                    confirmed_at: None,
                    error: None,
                };
                self.pending_submissions.lock().await.push(record);
            }
            Err(e) => {
                tracing::error!("Failed to submit attestation to Solana: {}", e);
                
                // Track failure
                let record = AttestationRecord {
                    attestation: attestation.clone(),
                    deposit_id: 0,
                    status: AttestationStatus::Failed(e.to_string()),
                    solana_signature: None,
                    submitted_at: None,
                    confirmed_at: None,
                    error: Some(e.to_string()),
                };
                self.pending_submissions.lock().await.push(record);
                
                return Err(e);
            }
        }

        Ok(())
    }

    /// Get submission statistics
    pub async fn get_stats(&self) -> AttestationStats {
        let submissions = self.pending_submissions.lock().await;
        
        let submitted = submissions.iter()
            .filter(|r| matches!(r.status, AttestationStatus::Submitted))
            .count();
        let confirmed = submissions.iter()
            .filter(|r| matches!(r.status, AttestationStatus::Confirmed))
            .count();
        let failed = submissions.iter()
            .filter(|r| matches!(r.status, AttestationStatus::Failed(_)))
            .count();

        AttestationStats {
            total_processed: submissions.len(),
            submitted,
            confirmed,
            failed,
        }
    }
}

/// Statistics about attestation processing
#[derive(Debug, Clone)]
pub struct AttestationStats {
    pub total_processed: usize,
    pub submitted: usize,
    pub confirmed: usize,
    pub failed: usize,
}

/// Get current unix timestamp
fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = AttestationServiceConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(10));
        assert_eq!(config.max_retries, 3);
    }
}
