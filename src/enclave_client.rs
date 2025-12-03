//! Enclave HTTP Client
//! 
//! Communicates with the zcash-enclave REST API to:
//! - Fetch pending attestations
//! - Mark attestations as submitted

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::types::EnclaveAttestation;

/// HTTP client for the zcash-enclave service
pub struct EnclaveClient {
    client: Client,
    base_url: String,
}

/// Response from GET /v1/status
#[derive(Debug, Deserialize)]
pub struct EnclaveStatusResponse {
    pub provisioned: bool,
    pub enclave_pubkey: String,
    pub pending_attestations: usize,
    pub last_scanned_height: u64,
}

/// Request for POST /v1/attestations/mark-submitted
#[derive(Debug, Serialize)]
struct MarkSubmittedRequest {
    note_commitment: String,
}

/// Response from POST /v1/scan
#[derive(Debug, Deserialize)]
pub struct ScanResultResponse {
    pub blocks_scanned: u64,
    pub deposits_found: usize,
    pub attestations: Vec<EnclaveAttestation>,
}

/// Request for POST /v1/provision
#[derive(Debug, Serialize)]
pub struct ProvisionRequest {
    pub ufvk: String,
    pub bridge_ua: String,
}

/// Response from POST /v1/provision
#[derive(Debug, Deserialize)]
pub struct ProvisionResponse {
    pub enclave_pubkey: String,
    pub status: String,
}

impl EnclaveClient {
    /// Create a new enclave client
    pub fn new(base_url: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Health check - verify enclave is reachable
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        let response = self.client.get(&url).send().await?;
        Ok(response.status().is_success())
    }

    /// Get enclave status including provisioning state and pending attestations count
    pub async fn get_status(&self) -> Result<EnclaveStatusResponse> {
        let url = format!("{}/v1/status", self.base_url);
        let response = self.client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch enclave status")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Enclave status request failed: {} - {}", status, body);
        }

        let status: EnclaveStatusResponse = response.json().await?;
        Ok(status)
    }

    /// Fetch all pending attestations that need to be submitted to Solana
    pub async fn get_pending_attestations(&self) -> Result<Vec<EnclaveAttestation>> {
        let url = format!("{}/v1/attestations/pending", self.base_url);
        let response = self.client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch pending attestations")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Get pending attestations failed: {} - {}", status, body);
        }

        let attestations: Vec<EnclaveAttestation> = response.json().await?;
        Ok(attestations)
    }

    /// Mark an attestation as submitted (removes it from the pending queue)
    pub async fn mark_attestation_submitted(&self, note_commitment: &str) -> Result<()> {
        let url = format!("{}/v1/attestations/mark-submitted", self.base_url);
        let request = MarkSubmittedRequest {
            note_commitment: note_commitment.to_string(),
        };

        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to mark attestation as submitted")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Mark submitted failed: {} - {}", status, body);
        }

        Ok(())
    }

    /// Trigger a manual scan (for testing/recovery)
    pub async fn trigger_scan(&self, start_height: u64, end_height: u64) -> Result<ScanResultResponse> {
        let url = format!("{}/v1/scan", self.base_url);
        let request = serde_json::json!({
            "start_height": start_height,
            "end_height": end_height,
        });

        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to trigger scan")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Scan request failed: {} - {}", status, body);
        }

        let result: ScanResultResponse = response.json().await?;
        Ok(result)
    }

    /// Provision the enclave with UFVK from DKG
    pub async fn provision(&self, ufvk: &str, bridge_ua: &str) -> Result<ProvisionResponse> {
        let url = format!("{}/v1/provision", self.base_url);
        let request = ProvisionRequest {
            ufvk: ufvk.to_string(),
            bridge_ua: bridge_ua.to_string(),
        };

        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to provision enclave")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Provision failed: {} - {}", status, body);
        }

        let result: ProvisionResponse = response.json().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = EnclaveClient::new("http://localhost:8081");
        assert_eq!(client.base_url, "http://localhost:8081");
    }

    #[test]
    fn test_url_trailing_slash() {
        let client = EnclaveClient::new("http://localhost:8081/");
        assert_eq!(client.base_url, "http://localhost:8081");
    }
}