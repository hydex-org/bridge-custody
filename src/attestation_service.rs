//! Attestation Service
//! 
//! Continuously polls the enclave for pending attestations
//! and submits them to the Solana bridge program.

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::enclave_client::EnclaveClient;
use crate::solana_client::BridgeSolanaClient;
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

/// Service that bridges enclave attestations to Solana
pub struct AttestationService {
    enclave_client: EnclaveClient,
    solana_client: Arc<BridgeSolanaClient>,
    config: AttestationServiceConfig,
    /// Track in-flight attestations
    pending_submissions: Arc<Mutex<Vec<AttestationRecord>>>,
}

impl AttestationService {
    /// Create a new attestation service
    pub fn new(
        enclave_url: &str,
        solana_rpc_url: &str,
        program_id: &str,
        keypair_path: &str,
        config: AttestationServiceConfig,
    ) -> Result<Self> {
        let enclave_client = EnclaveClient::new(enclave_url);
        let solana_client = BridgeSolanaClient::new(solana_rpc_url, program_id, keypair_path)?;

        Ok(Self {
            enclave_client,
            solana_client: Arc::new(solana_client),
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

        // Check Solana
        tracing::info!("Checking Solana connection...");
        let balance = self.solana_client.check_balance().await?;
        tracing::info!("   Solana: OK (payer balance: {} lamports)", balance);

        if balance < 10_000_000 {
            tracing::warn!("   Low payer balance! Consider funding: {}", self.solana_client.payer_pubkey());
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

    /// Process a single attestation
    async fn process_single_attestation(&self, attestation: &EnclaveAttestation) -> Result<()> {
        tracing::info!(
            "Processing attestation: amount={} zatoshi, block={}, recipient={}...",
            attestation.amount,
            attestation.block_height,
            &attestation.recipient_solana[..16]
        );

        // 1. Check if note already claimed (prevent double-mint)
        let note_commitment = attestation.note_commitment_bytes()?;
        if self.solana_client.is_note_claimed(&note_commitment).await? {
            tracing::warn!("Note already claimed, marking as submitted");
            self.enclave_client.mark_attestation_submitted(&attestation.note_commitment).await?;
            return Ok(());
        }

        // 2. Parse recipient Solana pubkey
        let recipient_bytes = attestation.recipient_solana_bytes()?;
        let recipient = solana_sdk::pubkey::Pubkey::try_from(recipient_bytes.as_slice())?;

        // 3. Find deposit_id for this recipient
        // In production, this would query the enclave or a mapping service
        // For now, we use a placeholder
        let deposit_id = self.find_deposit_id_for_attestation(attestation).await?;

        // 4. Submit attestation to Solana
        let result = self.solana_client
            .submit_attestation(attestation, deposit_id, &recipient)
            .await;

        match result {
            Ok(submit_result) => {
                tracing::info!(
                    "Attestation submitted successfully: {}",
                    submit_result.signature
                );
                
                // 5. Mark as submitted in enclave
                self.enclave_client
                    .mark_attestation_submitted(&attestation.note_commitment)
                    .await?;

                // Track submission
                let record = AttestationRecord {
                    attestation: attestation.clone(),
                    deposit_id,
                    status: AttestationStatus::Submitted,
                    solana_signature: Some(submit_result.signature),
                    submitted_at: Some(chrono_timestamp()),
                    confirmed_at: None,
                    error: None,
                };
                self.pending_submissions.lock().await.push(record);
            }
            Err(e) => {
                tracing::error!("Failed to submit attestation: {}", e);
                
                // Track failure
                let record = AttestationRecord {
                    attestation: attestation.clone(),
                    deposit_id,
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

    /// Find the deposit_id associated with an attestation
    /// 
    /// This requires querying the enclave's UA -> deposit_id mapping
    /// or matching by recipient_solana pubkey
    async fn find_deposit_id_for_attestation(&self, attestation: &EnclaveAttestation) -> Result<u64> {
        // In a full implementation, this would:
        // 1. Query enclave for UA -> deposit_id mapping
        // 2. Or query Solana for DepositIntent by user pubkey
        // 
        // For now, we use a placeholder approach:
        // The deposit_id should be passed along with the attestation from the enclave
        
        // TODO: Update enclave API to include deposit_id in attestation response
        tracing::warn!("Using placeholder deposit_id=0 - implement proper lookup");
        Ok(0)
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