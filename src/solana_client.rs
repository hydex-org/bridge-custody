//! Solana Client for Bridge Program Interactions
//! 
//! Uses raw JSON-RPC via reqwest instead of solana-client to avoid
//! heavy transitive dependencies and version conflicts.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::types::EnclaveAttestation;

/// Solana client using raw JSON-RPC
pub struct BridgeSolanaClient {
    client: Client,
    rpc_url: String,
    program_id: Pubkey,
    payer: Arc<Keypair>,
}

/// Result of submitting an attestation
#[derive(Debug, Clone)]
pub struct AttestationSubmitResult {
    pub signature: String,
    pub deposit_id: u64,
    pub success: bool,
}

/// Deposit intent account data (simplified)
#[derive(Debug, Clone)]
pub struct DepositIntentAccount {
    pub deposit_id: u64,
    pub user: Pubkey,
    pub diversifier_index: u32,
    pub status: u8,
    pub amount: u64,
    pub note_commitment: [u8; 32],
}

// JSON-RPC request/response types
#[derive(Serialize)]
struct RpcRequest<T: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: T,
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize, Debug)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct GetBalanceResult {
    value: u64,
}

#[derive(Deserialize)]
struct GetLatestBlockhashResult {
    value: BlockhashValue,
}

#[derive(Deserialize)]
struct BlockhashValue {
    blockhash: String,
}

#[derive(Deserialize)]
struct SendTransactionResult(String);

#[derive(Deserialize)]
struct GetAccountInfoResult {
    value: Option<AccountValue>,
}

#[derive(Deserialize)]
struct AccountValue {
    data: Vec<String>,
    lamports: u64,
}

impl BridgeSolanaClient {
    /// Create a new Solana client
    pub fn new(rpc_url: &str, program_id: &str, keypair_path: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        let program_id = Pubkey::from_str(program_id)
            .context("Invalid program ID")?;

        // Load or generate payer keypair
        let payer = if std::path::Path::new(keypair_path).exists() {
            let keypair_bytes = std::fs::read(keypair_path)?;
            let keypair_json: Vec<u8> = serde_json::from_slice(&keypair_bytes)?;
            Keypair::try_from(keypair_json.as_slice())
                .context("Invalid keypair file")?
        } else {
            tracing::warn!("Keypair file not found at {}, generating new keypair", keypair_path);
            let keypair = Keypair::new();
            // Save the new keypair
            if let Some(parent) = std::path::Path::new(keypair_path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            let keypair_json = serde_json::to_vec(&keypair.to_bytes().to_vec())?;
            std::fs::write(keypair_path, keypair_json)?;
            tracing::info!("Generated and saved new keypair to {}", keypair_path);
            tracing::info!("Payer pubkey: {}", keypair.pubkey());
            keypair
        };

        tracing::info!("Solana client initialized");
        tracing::info!("   RPC: {}", rpc_url);
        tracing::info!("   Program: {}", program_id);
        tracing::info!("   Payer: {}", payer.pubkey());

        Ok(Self {
            client,
            rpc_url: rpc_url.to_string(),
            program_id,
            payer: Arc::new(payer),
        })
    }

    /// Get payer public key
    pub fn payer_pubkey(&self) -> Pubkey {
        self.payer.pubkey()
    }

    /// Check payer balance via RPC
    pub async fn check_balance(&self) -> Result<u64> {
        let params = serde_json::json!([
            self.payer.pubkey().to_string(),
            {"commitment": "confirmed"}
        ]);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getBalance",
            params,
        };

        let response: RpcResponse<GetBalanceResult> = self.client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        if let Some(error) = response.error {
            anyhow::bail!("RPC error: {} (code {})", error.message, error.code);
        }

        Ok(response.result.map(|r| r.value).unwrap_or(0))
    }

    /// Get latest blockhash
    async fn get_latest_blockhash(&self) -> Result<Hash> {
        let params = serde_json::json!([{"commitment": "confirmed"}]);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getLatestBlockhash",
            params,
        };

        let response: RpcResponse<GetLatestBlockhashResult> = self.client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        if let Some(error) = response.error {
            anyhow::bail!("RPC error: {} (code {})", error.message, error.code);
        }

        let blockhash_str = response.result
            .ok_or_else(|| anyhow::anyhow!("No blockhash in response"))?
            .value
            .blockhash;

        Hash::from_str(&blockhash_str)
            .context("Invalid blockhash format")
    }

    /// Send a signed transaction
    async fn send_transaction(&self, transaction: &Transaction) -> Result<Signature> {
        let tx_bytes = bincode::serialize(transaction)?;
        let tx_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &tx_bytes);

        let params = serde_json::json!([
            tx_base64,
            {
                "encoding": "base64",
                "skipPreflight": false,
                "preflightCommitment": "confirmed"
            }
        ]);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "sendTransaction",
            params,
        };

        let response: RpcResponse<String> = self.client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        if let Some(error) = response.error {
            anyhow::bail!("Transaction failed: {} (code {})", error.message, error.code);
        }

        let sig_str = response.result
            .ok_or_else(|| anyhow::anyhow!("No signature in response"))?;

        Signature::from_str(&sig_str)
            .context("Invalid signature format")
    }

    /// Get account info
    async fn get_account_data(&self, pubkey: &Pubkey) -> Result<Option<Vec<u8>>> {
        let params = serde_json::json!([
            pubkey.to_string(),
            {
                "encoding": "base64",
                "commitment": "confirmed"
            }
        ]);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getAccountInfo",
            params,
        };

        let response: RpcResponse<GetAccountInfoResult> = self.client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        if let Some(error) = response.error {
            anyhow::bail!("RPC error: {} (code {})", error.message, error.code);
        }

        if let Some(result) = response.result {
            if let Some(account) = result.value {
                if !account.data.is_empty() {
                    let data = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        &account.data[0]
                    )?;
                    return Ok(Some(data));
                }
            }
        }

        Ok(None)
    }

    /// Derive bridge config PDA
    pub fn derive_bridge_config_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"bridge-config"],
            &self.program_id,
        )
    }

    /// Derive deposit intent PDA
    pub fn derive_deposit_intent_pda(&self, user: &Pubkey, deposit_id: u64) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                b"deposit-intent",
                user.as_ref(),
                &deposit_id.to_le_bytes(),
            ],
            &self.program_id,
        )
    }

    /// Derive claim tracker PDA (prevents double-minting)
    pub fn derive_claim_tracker_pda(&self, note_commitment: &[u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"claim-tracker", note_commitment.as_ref()],
            &self.program_id,
        )
    }

    /// Submit an attestation to mint private tokens
    pub async fn submit_attestation(
        &self,
        attestation: &EnclaveAttestation,
        deposit_id: u64,
        user: &Pubkey,
    ) -> Result<AttestationSubmitResult> {
        tracing::info!(
            "Submitting attestation for deposit {} (amount: {} zatoshi)",
            deposit_id,
            attestation.amount
        );

        // Serialize attestation to bytes (176 bytes total)
        let attestation_bytes = attestation.to_attestation_bytes()?;

        // Split into 32-byte chunks for Arcium encryption
        let mut encrypted_attestation: Vec<[u8; 32]> = Vec::new();
        for chunk in attestation_bytes.chunks(32) {
            let mut arr = [0u8; 32];
            let len = chunk.len().min(32);
            arr[..len].copy_from_slice(&chunk[..len]);
            encrypted_attestation.push(arr);
        }

        // Placeholder encryption params (set by MXE in production)
        let pub_key = [0u8; 32];
        let nonce: u128 = rand::random();

        // Build instruction data
        let discriminator = Self::get_instruction_discriminator("mint_private_with_attestation");
        
        let mut data = Vec::new();
        data.extend_from_slice(&discriminator);
        
        let computation_offset: u64 = 0;
        data.extend_from_slice(&computation_offset.to_le_bytes());
        data.extend_from_slice(&deposit_id.to_le_bytes());
        
        // Vec<[u8; 32]> with length prefix
        data.extend_from_slice(&(encrypted_attestation.len() as u32).to_le_bytes());
        for chunk in &encrypted_attestation {
            data.extend_from_slice(chunk);
        }
        
        data.extend_from_slice(&pub_key);
        data.extend_from_slice(&nonce.to_le_bytes());

        // Derive PDAs
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        let (deposit_intent_pda, _) = self.derive_deposit_intent_pda(user, deposit_id);

        // Build accounts
        let accounts = vec![
            AccountMeta::new(*user, false),
            AccountMeta::new_readonly(bridge_config_pda, false),
            AccountMeta::new(deposit_intent_pda, false),
            AccountMeta::new(self.payer.pubkey(), true),
        ];

        let instruction = Instruction {
            program_id: self.program_id,
            accounts,
            data,
        };

        // Build and send transaction
        let recent_blockhash = self.get_latest_blockhash().await?;
        
        let message = Message::new(&[instruction], Some(&self.payer.pubkey()));
        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&[&*self.payer], recent_blockhash);

        match self.send_transaction(&transaction).await {
            Ok(signature) => {
                tracing::info!("Attestation submitted: {}", signature);
                Ok(AttestationSubmitResult {
                    signature: signature.to_string(),
                    deposit_id,
                    success: true,
                })
            }
            Err(e) => {
                tracing::error!("Failed to submit attestation: {}", e);
                Err(e)
            }
        }
    }

    /// Check if a note has already been claimed
    pub async fn is_note_claimed(&self, note_commitment: &[u8; 32]) -> Result<bool> {
        let (claim_tracker_pda, _) = self.derive_claim_tracker_pda(note_commitment);
        
        match self.get_account_data(&claim_tracker_pda).await {
            Ok(Some(_)) => Ok(true),   // Account exists = note claimed
            Ok(None) => Ok(false),      // Account doesn't exist = not claimed
            Err(_) => Ok(false),        // Error fetching = assume not claimed
        }
    }

    /// Get Anchor instruction discriminator
    fn get_instruction_discriminator(name: &str) -> [u8; 8] {
        use sha2::{Sha256, Digest};
        let preimage = format!("global:{}", name);
        let hash = Sha256::digest(preimage.as_bytes());
        let mut discriminator = [0u8; 8];
        discriminator.copy_from_slice(&hash[..8]);
        discriminator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_discriminator() {
        let disc = BridgeSolanaClient::get_instruction_discriminator("init_intent");
        assert_eq!(disc.len(), 8);
    }

    #[test]
    fn test_pda_derivation() {
        let program_id = Pubkey::from_str("HefTNtytDcQgSQmBpPuwjGipbVcJTMRHnppU9poWRXhD").unwrap();
        
        let (bridge_config, _) = Pubkey::find_program_address(
            &[b"bridge-config"],
            &program_id,
        );
        
        assert!(!bridge_config.to_string().is_empty());
    }
}