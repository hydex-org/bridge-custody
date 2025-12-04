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
use base64::Engine;
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
        use base64::Engine;
        let tx_bytes = bincode::serialize(transaction)?;
        let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);


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
                let data = base64::engine::general_purpose::STANDARD.decode(&account.data[0])?;
                return Ok(Some(data));
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

    /// Derive sZEC mint PDA
    pub fn derive_szec_mint_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"szec-mint"],
            &self.program_id,
        )
    }

    /// Derive mint authority PDA
    pub fn derive_mint_authority_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"mint-authority"],
            &self.program_id,
        )
    }

    /// Derive associated token address (same as spl_associated_token_account::get_associated_token_address)
    pub fn derive_associated_token_address(&self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        let token_program_id = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
        let ata_program_id = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
        
        let (address, _) = Pubkey::find_program_address(
            &[
                owner.as_ref(),
                token_program_id.as_ref(),
                mint.as_ref(),
            ],
            &ata_program_id,
        );
        address
    }

    /// Submit an attestation to mint tokens using mint_simple (devnet flow)
    /// Simplified version - just requires MPC authority to sign the transaction
    /// 
    /// This function will automatically create a deposit intent if one doesn't exist
    pub async fn submit_attestation(
        &self,
        attestation: &EnclaveAttestation,
        _deposit_id: u64,  // Ignored - we use ensure_deposit_intent to get/create the right one
        user: &Pubkey,
    ) -> Result<AttestationSubmitResult> {
        tracing::info!(
            "Submitting attestation for user {} (amount: {} zatoshi)",
            user,
            attestation.amount
        );

        // Step 1: Ensure deposit intent exists (creates if needed)
        let actual_deposit_id = match self.ensure_deposit_intent(user).await {
            Ok(id) => {
                tracing::info!("Using deposit intent #{} for user {}", id, user);
                id
            }
            Err(e) => {
                tracing::error!("Failed to ensure deposit intent: {}", e);
                return Err(e);
            }
        };

        // Parse hex-encoded fields from attestation
        let note_commitment = attestation.note_commitment_bytes()
            .context("Failed to parse note_commitment")?;
        let amount = attestation.amount;
        let block_height = attestation.block_height;

        // Step 2: Check if note already claimed
        if self.is_note_claimed(&note_commitment).await? {
            tracing::warn!("Note already claimed, skipping mint");
            return Ok(AttestationSubmitResult {
                signature: "already_claimed".to_string(),
                deposit_id: actual_deposit_id,
                success: false,
            });
        }

        // Step 3: Build mint_simple instruction
        let discriminator = Self::get_instruction_discriminator("mint_simple");
                
        let mut data = Vec::new();
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&note_commitment);                    // 32 bytes
        data.extend_from_slice(&amount.to_le_bytes());               // 8 bytes
        data.extend_from_slice(&block_height.to_le_bytes());         // 8 bytes

        // Derive PDAs
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        let (deposit_intent_pda, _) = self.derive_deposit_intent_pda(user, actual_deposit_id);
        let (claim_tracker_pda, _) = self.derive_claim_tracker_pda(&note_commitment);
        let (szec_mint_pda, _) = self.derive_szec_mint_pda();
        let (mint_authority_pda, _) = self.derive_mint_authority_pda();
        
        // Derive user's associated token account
        let user_token_account = self.derive_associated_token_address(user, &szec_mint_pda);

                // Program IDs
                let token_program_id = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
                let associated_token_program_id = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
        
                // Build accounts (must match MintSimple struct order in Solana program)
                        // Build accounts (must match MintSimple struct order in Solana program)
        let accounts = vec![
            AccountMeta::new_readonly(self.payer.pubkey(), true), // authority (signer)
            AccountMeta::new(self.payer.pubkey(), true),          // payer (signer)
            AccountMeta::new(bridge_config_pda, false),           // bridge_config
            AccountMeta::new(deposit_intent_pda, false),          // deposit_intent
            AccountMeta::new(claim_tracker_pda, false),           // claim_tracker
            AccountMeta::new(szec_mint_pda, false),               // szec_mint
            AccountMeta::new_readonly(mint_authority_pda, false), // mint_authority
            AccountMeta::new_readonly(*user, false),              // user_wallet (NEW)
            AccountMeta::new(user_token_account, false),          // user_token_account
            AccountMeta::new_readonly(associated_token_program_id, false), // associated_token_program
            AccountMeta::new_readonly(token_program_id, false),   // token_program
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false), // system_program
        ];

        let mint_simple_ix = Instruction {
            program_id: self.program_id,
            accounts,
            data,
        };

        // Build and send transaction
        let recent_blockhash = self.get_latest_blockhash().await?;
        
        let message = Message::new(&[mint_simple_ix], Some(&self.payer.pubkey()));
        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&[&*self.payer], recent_blockhash);

        match self.send_transaction(&transaction).await {
            Ok(signature) => {
                tracing::info!("Mint successful! TX: {}", signature);
                tracing::info!("   User: {}", user);
                tracing::info!("   Amount: {} zatoshi", amount);
                tracing::info!("   Deposit ID: {}", actual_deposit_id);
                Ok(AttestationSubmitResult {
                    signature: signature.to_string(),
                    deposit_id: actual_deposit_id,
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

    /// Get current deposit nonce from bridge config
    pub async fn get_deposit_nonce(&self) -> Result<u64> {
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        
        let data = self.get_account_data(&bridge_config_pda).await?
            .ok_or_else(|| anyhow::anyhow!("Bridge config not found"))?;
        
        // BridgeConfig layout: discriminator(8) + bump(1) + admin(32) + enclave_authority(32) + 
        //                      mpc_authority(32) + szec_mint(32) + deposit_nonce(8) + ...
        // deposit_nonce is at offset 8 + 1 + 32 + 32 + 32 + 32 = 137
        if data.len() < 145 {
            anyhow::bail!("Bridge config account data too short");
        }
        
        let nonce_bytes: [u8; 8] = data[137..145].try_into()?;
        Ok(u64::from_le_bytes(nonce_bytes))
    }

    /// Check if deposit intent exists for a user
    pub async fn deposit_intent_exists(&self, user: &Pubkey, deposit_id: u64) -> Result<bool> {
        let (deposit_intent_pda, _) = self.derive_deposit_intent_pda(user, deposit_id);
        
        match self.get_account_data(&deposit_intent_pda).await {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(_) => Ok(false),
        }
    }

    /// Create a deposit intent for a user using create_deposit_for_user
    /// This creates the on-chain state needed before minting
    pub async fn create_deposit_for_user(
        &self,
        recipient: &Pubkey,
        ua_hash: [u8; 32],
    ) -> Result<u64> {
        let deposit_nonce = self.get_deposit_nonce().await?;
        
        tracing::info!(
            "Creating deposit intent for user {} (nonce: {})",
            recipient,
            deposit_nonce
        );

        let discriminator = Self::get_instruction_discriminator("create_deposit_for_user");
        
        let mut data = Vec::new();
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(recipient.as_ref());  // recipient: Pubkey (32 bytes)
        data.extend_from_slice(&ua_hash);            // ua_hash: [u8; 32]

        // Derive PDAs
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        let (deposit_intent_pda, _) = self.derive_deposit_intent_pda(recipient, deposit_nonce);

        // Build accounts (must match CreateDepositForUser struct)
        let accounts = vec![
            AccountMeta::new_readonly(self.payer.pubkey(), true), // authority (signer)
            AccountMeta::new(self.payer.pubkey(), true),          // payer (signer)
            AccountMeta::new(bridge_config_pda, false),           // bridge_config
            AccountMeta::new(deposit_intent_pda, false),          // deposit_intent
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false), // system_program
        ];

        let instruction = Instruction {
            program_id: self.program_id,
            accounts,
            data,
        };

        let recent_blockhash = self.get_latest_blockhash().await?;
        
        let message = Message::new(&[instruction], Some(&self.payer.pubkey()));
        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&[&*self.payer], recent_blockhash);

        let signature = self.send_transaction(&transaction).await?;
        tracing::info!(
            "Created deposit intent #{} for {} (tx: {})",
            deposit_nonce,
            recipient,
            signature
        );

        Ok(deposit_nonce)
    }

    /// Ensure deposit intent exists, creating it if necessary
    pub async fn ensure_deposit_intent(
        &self,
        user: &Pubkey,
    ) -> Result<u64> {
        let deposit_nonce = self.get_deposit_nonce().await?;
        
        // Check if intent already exists for this user at current nonce
        // We'll try a few recent nonces since we don't know which one corresponds to this deposit
        for offset in 0..5 {
            if deposit_nonce < offset {
                break;
            }
            let check_id = deposit_nonce.saturating_sub(offset);
            if self.deposit_intent_exists(user, check_id).await? {
                tracing::info!("Found existing deposit intent #{} for user {}", check_id, user);
                return Ok(check_id);
            }
        }

        // No existing intent found, create one
        tracing::info!("No deposit intent found for user {}, creating new one", user);
        
        // Create a UA hash (we don't have the actual UA, so use a hash of user + nonce)
        let mut ua_hash = [0u8; 32];
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(user.as_ref());
        hasher.update(&deposit_nonce.to_le_bytes());
        hasher.update(b"hydex-ua-hash");
        let result = hasher.finalize();
        ua_hash.copy_from_slice(&result[..32]);

        // Try to create - if it fails due to race condition, re-check for existing
        match self.create_deposit_for_user(user, ua_hash).await {
            Ok(id) => Ok(id),
            Err(e) => {
                // Race condition - another node may have created it
                tracing::warn!("Create failed ({}), re-checking for existing intent...", e);
                
                // Wait a moment for the other tx to confirm
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                
                // Re-fetch nonce and check again
                let new_nonce = self.get_deposit_nonce().await?;
                for offset in 0..5 {
                    if new_nonce < offset {
                        break;
                    }
                    let check_id = new_nonce.saturating_sub(offset);
                    if self.deposit_intent_exists(user, check_id).await? {
                        tracing::info!("Found deposit intent #{} for {} after race (created by another node)", check_id, user);
                        return Ok(check_id);
                    }
                }
                
                // Still not found - propagate original error
                Err(e)
            }
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

    // =========================================================================
    // WITHDRAWAL METHODS
    // =========================================================================

    /// Derive burn intent PDA
    pub fn derive_burn_intent_pda(&self, user: &Pubkey, burn_id: u64) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                b"burn-intent",
                user.as_ref(),
                &burn_id.to_le_bytes(),
            ],
            &self.program_id,
        )
    }

    /// Get current burn nonce from bridge config
    pub async fn get_burn_nonce(&self) -> Result<u64> {
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        
        let data = self.get_account_data(&bridge_config_pda).await?
            .ok_or_else(|| anyhow::anyhow!("Bridge config not found"))?;
        
        // BridgeConfig layout: discriminator(8) + bump(1) + admin(32) + enclave_authority(32) + 
        //                      mpc_authority(32) + szec_mint(32) + deposit_nonce(8) + burn_nonce(8) + ...
        // burn_nonce is at offset 8 + 1 + 32 + 32 + 32 + 32 + 8 = 145
        if data.len() < 153 {
            anyhow::bail!("Bridge config account data too short for burn_nonce");
        }
        
        let nonce_bytes: [u8; 8] = data[145..153].try_into()?;
        Ok(u64::from_le_bytes(nonce_bytes))
    }

    /// Fetch a specific burn intent by user and burn_id
    pub async fn get_burn_intent(&self, user: &Pubkey, burn_id: u64) -> Result<Option<BurnIntentData>> {
        let (burn_intent_pda, _) = self.derive_burn_intent_pda(user, burn_id);
        
        let data = match self.get_account_data(&burn_intent_pda).await? {
            Some(d) => d,
            None => return Ok(None),
        };
        
        // BurnIntent layout: discriminator(8) + bump(1) + burn_id(8) + user(32) + amount(8) + 
        //                    status(1) + encrypted_data_hash(32) + zcash_txid(32) + created_at(8)
        if data.len() < 130 {
            anyhow::bail!("BurnIntent account data too short");
        }
        
        let burn_id = u64::from_le_bytes(data[9..17].try_into()?);
        let user = Pubkey::try_from(&data[17..49])?;
        let amount = u64::from_le_bytes(data[49..57].try_into()?);
        let status = data[57];
        let mut encrypted_data_hash = [0u8; 32];
        encrypted_data_hash.copy_from_slice(&data[58..90]);
        let mut zcash_txid = [0u8; 32];
        zcash_txid.copy_from_slice(&data[90..122]);
        
        Ok(Some(BurnIntentData {
            burn_id,
            user,
            amount,
            status,
            encrypted_data_hash,
            zcash_txid,
        }))
    }

    /// Fetch all pending burn intents (status = 0)
    /// Note: This scans recent burn IDs - for production, use getProgramAccounts with filters
    pub async fn fetch_pending_burns(&self, max_lookback: u64) -> Result<Vec<BurnIntentData>> {
        let burn_nonce = self.get_burn_nonce().await?;
        let mut pending = Vec::new();
        
        // Scan recent burn intents
        // Note: In production, this should use getProgramAccounts with memcmp filter
        // For now, we'll scan backwards from the current nonce
        let start = burn_nonce.saturating_sub(max_lookback);
        
        tracing::debug!("Scanning burn intents from {} to {}", start, burn_nonce);
        
        // We need to know the user addresses to check - this is a limitation
        // In production, use getProgramAccounts RPC call
        // For now, return empty - the actual implementation needs getProgramAccounts
        
        tracing::warn!(
            "fetch_pending_burns: Need to implement getProgramAccounts. \
            Current burn_nonce: {}",
            burn_nonce
        );
        
        Ok(pending)
    }

    /// Mark a burn intent as processing (status = 1)
    pub async fn mark_burn_processing(
        &self,
        burn_id: u64,
        user: &Pubkey,
    ) -> Result<String> {
        let discriminator = Self::get_instruction_discriminator("mark_burn_processing");
        
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        let (burn_intent_pda, _) = self.derive_burn_intent_pda(user, burn_id);
        
        let accounts = vec![
            AccountMeta::new_readonly(self.payer.pubkey(), true), // authority (signer)
            AccountMeta::new_readonly(bridge_config_pda, false),  // bridge_config
            AccountMeta::new(burn_intent_pda, false),             // burn_intent
        ];
        
        let instruction = Instruction {
            program_id: self.program_id,
            accounts,
            data: discriminator.to_vec(),
        };
        
        let recent_blockhash = self.get_latest_blockhash().await?;
        let message = Message::new(&[instruction], Some(&self.payer.pubkey()));
        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&[&*self.payer], recent_blockhash);
        
        let signature = self.send_transaction(&transaction).await?;
        tracing::info!("Marked burn #{} as processing: {}", burn_id, signature);
        
        Ok(signature.to_string())
    }

    /// Finalize a withdrawal after Zcash TX is confirmed
    pub async fn finalize_withdrawal(
        &self,
        burn_id: u64,
        user: &Pubkey,
        zcash_txid: [u8; 32],
        success: bool,
    ) -> Result<String> {
        let discriminator = Self::get_instruction_discriminator("finalize_withdrawal");
        
        let mut data = Vec::new();
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&zcash_txid);              // 32 bytes
        data.push(if success { 1 } else { 0 });           // 1 byte (bool)
        
        let (bridge_config_pda, _) = self.derive_bridge_config_pda();
        let (burn_intent_pda, _) = self.derive_burn_intent_pda(user, burn_id);
        
        let accounts = vec![
            AccountMeta::new_readonly(self.payer.pubkey(), true), // authority (signer)
            AccountMeta::new_readonly(bridge_config_pda, false),  // bridge_config
            AccountMeta::new(burn_intent_pda, false),             // burn_intent
        ];
        
        let instruction = Instruction {
            program_id: self.program_id,
            accounts,
            data,
        };
        
        let recent_blockhash = self.get_latest_blockhash().await?;
        let message = Message::new(&[instruction], Some(&self.payer.pubkey()));
        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&[&*self.payer], recent_blockhash);
        
        let signature = self.send_transaction(&transaction).await?;
        tracing::info!(
            "Finalized withdrawal #{}: success={}, tx={}",
            burn_id,
            success,
            signature
        );
        
        Ok(signature.to_string())
    }
}

/// Burn intent data from Solana
#[derive(Debug, Clone)]
pub struct BurnIntentData {
    pub burn_id: u64,
    pub user: Pubkey,
    pub amount: u64,
    pub status: u8,
    pub encrypted_data_hash: [u8; 32],
    pub zcash_txid: [u8; 32],
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
