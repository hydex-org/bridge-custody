use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use base64::{Engine as _, engine::general_purpose};

#[derive(Serialize)]
struct RpcRequest {
    jsonrpc: String,
    id: String,
    method: String,
    params: Vec<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize, Debug)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Deserialize, Debug)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    #[serde(rename = "bestblockhash")]
    pub best_block_hash: String,
    pub difficulty: f64,
    #[serde(rename = "verificationprogress")]
    pub verification_progress: f64,
}

pub struct ZcashRpcClient {
    client: Client,
    url: String,
    auth_header: String,
}

impl ZcashRpcClient {
    pub fn new(url: String, username: String, password: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let credentials = format!("{}:{}", username, password);
        let auth_header = format!("Basic {}", general_purpose::STANDARD.encode(credentials));

        Ok(Self {
            client,
            url,
            auth_header,
        })
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<T> {
        let request = RpcRequest {
            jsonrpc: "1.0".to_string(),
            id: "zcash-bridge".to_string(),
            method: method.to_string(),
            params,
        };

        let response = self.client
            .post(&self.url)
            .header("Content-Type", "text/plain")
            .header("Authorization", &self.auth_header)
            .json(&request)
            .send()
            .await
            .context("Failed to send RPC request")?;

        if !response.status().is_success() {
            bail!("RPC request failed with status: {}", response.status());
        }

        let rpc_response: RpcResponse<T> = response
            .json()
            .await
            .context("Failed to parse RPC response")?;

        if let Some(error) = rpc_response.error {
            bail!("RPC error {}: {}", error.code, error.message);
        }

        rpc_response.result
            .context("RPC response missing result field")
    }

    // === Basic Node Information ===

    pub async fn get_blockchain_info(&self) -> Result<BlockchainInfo> {
        self.call("getblockchaininfo", vec![]).await
    }

    pub async fn get_wallet_info(&self) -> Result<serde_json::Value> {
        self.call("getwalletinfo", vec![]).await
    }

    pub async fn get_block_count(&self) -> Result<u64> {
        self.call("getblockcount", vec![]).await
    }

    // === Account-Based Methods (Following Your Flowchart) ===

    /// Step 1: Create a new unified account
    /// Step 1: Create a new unified account
    pub async fn z_getnewaccount(&self) -> Result<u32> {
        let response: serde_json::Value = self.call("z_getnewaccount", vec![]).await?;
        
        // Handle different response formats
        if let Some(account) = response.as_u64() {
            // Direct number response
            Ok(account as u32)
        } else if let Some(account) = response.get("account").and_then(|v| v.as_u64()) {
            // Object with "account" field
            Ok(account as u32)
        } else {
            Err(anyhow::anyhow!("Unexpected z_getnewaccount response format: {}", response))
        }
    }
    /// Step 2: Get unified address for an account
    pub async fn z_getaddressforaccount(
        &self,
        account: u32,
        diversifier_index: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut params = vec![serde_json::json!(account)];
        if let Some(div) = diversifier_index {
            params.push(serde_json::json!(div));
        }
        self.call("z_getaddressforaccount", params).await
    }

    /// Step 3: List all accounts
    pub async fn z_listaccounts(&self) -> Result<Vec<serde_json::Value>> {
        self.call("z_listaccounts", vec![]).await
    }

    /// Step 4: Get individual receivers (Orchard, Sapling, Transparent)
    pub async fn z_listunifiedreceivers(
        &self,
        unified_address: &str,
    ) -> Result<serde_json::Value> {
        self.call("z_listunifiedreceivers", vec![serde_json::json!(unified_address)]).await
    }

    /// Step 5: Get balance for an account
    pub async fn z_getbalanceforaccount(
        &self,
        account_index: u32,
        min_conf: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut params = vec![serde_json::json!(account_index)];
        if let Some(conf) = min_conf {
            params.push(serde_json::json!(conf));
        }
        self.call("z_getbalanceforaccount", params).await
    }

    /// Alternative: Get balance by address
    pub async fn z_getbalance(
        &self,
        address: &str,
        min_conf: Option<u32>,
    ) -> Result<f64> {
        let mut params = vec![serde_json::json!(address)];
        if let Some(conf) = min_conf {
            params.push(serde_json::json!(conf));
        }
        self.call("z_getbalance", params).await
    }

    // === Transaction Methods ===

    /// Send transaction
    pub async fn z_sendmany(
        &self,
        from_address: &str,
        recipients: Vec<(&str, f64)>,
        min_confirmations: u32,
        fee: Option<f64>,
    ) -> Result<String> {
        let recipients_json: Vec<_> = recipients
            .into_iter()
            .map(|(addr, amount)| {
                serde_json::json!({
                    "address": addr,
                    "amount": amount
                })
            })
            .collect();

        let mut params = vec![
            serde_json::json!(from_address),
            serde_json::json!(recipients_json),
            serde_json::json!(min_confirmations),
        ];

        if let Some(f) = fee {
            params.push(serde_json::json!(f));
        }

        let operation_id: String = self.call("z_sendmany", params).await?;
        Ok(operation_id)
    }

    /// Check operation status
    pub async fn z_getoperationstatus(
        &self,
        operation_ids: Option<Vec<String>>,
    ) -> Result<serde_json::Value> {
        let params = if let Some(ids) = operation_ids {
            vec![serde_json::json!(ids)]
        } else {
            vec![]
        };
        self.call("z_getoperationstatus", params).await
    }

    /// Get operation result
    pub async fn z_getoperationresult(
        &self,
        operation_ids: Option<Vec<String>>,
    ) -> Result<serde_json::Value> {
        let params = if let Some(ids) = operation_ids {
            vec![serde_json::json!(ids)]
        } else {
            vec![]
        };
        self.call("z_getoperationresult", params).await
    }

    /// Submit a raw transaction to the network
    /// Returns the transaction ID (txid) if successful
    pub async fn send_raw_transaction(&self, tx_hex: String) -> Result<String> {
        let txid: String = self.call(
            "sendrawtransaction",
            vec![serde_json::json!(tx_hex)]
        ).await?;
        
        Ok(txid)
    }

    /// Get information about a transaction
    /// If verbose is true (1), returns a JSON object with transaction details
    /// If verbose is false (0), returns only the raw transaction hex
    pub async fn get_raw_transaction(
        &self,
        txid: &str,
        verbose: bool,
    ) -> Result<serde_json::Value> {
        self.call(
            "getrawtransaction",
            vec![
                serde_json::json!(txid),
                serde_json::json!(if verbose { 1 } else { 0 }),
            ]
        ).await
    }

    /// Decode a raw transaction hex into a JSON object
    pub async fn decode_raw_transaction(&self, hex_string: &str) -> Result<serde_json::Value> {
        self.call(
            "decoderawtransaction",
            vec![serde_json::json!(hex_string)]
        ).await
    }

    /// Get transaction details from the wallet
    pub async fn get_transaction(&self, txid: &str) -> Result<serde_json::Value> {
        self.call(
            "gettransaction",
            vec![serde_json::json!(txid)]
        ).await
    }

    // === List Methods ===

    /// List unspent notes
    pub async fn z_listunspent(
        &self,
        min_conf: u32,
        max_conf: u32,
        include_watch_only: bool,
        addresses: Vec<String>,
    ) -> Result<serde_json::Value> {
        let params = vec![
            serde_json::json!(min_conf),
            serde_json::json!(max_conf),
            serde_json::json!(include_watch_only),
            serde_json::json!(addresses),
        ];
        self.call("z_listunspent", params).await
    }
    
    pub async fn listwallets(&self) -> Result<Vec<String>> {
        self.call("listwallets", vec![]).await
    }

    pub async fn getinfo(&self) -> Result<serde_json::Value> {
        self.call("getinfo", vec![]).await
    }

    pub async fn get_total_balance(&self) -> Result<serde_json::Value> {
        self.call("z_gettotalbalance", vec![]).await
    }
    
    /// Import a viewing key (UFVK) into the wallet
    /// 
    /// Parameters:
    /// - viewing_key: The Unified Full Viewing Key (UFVK) string
    /// - rescan: Optional rescan height (None = rescan from wallet birthday)
    /// - start_height: Optional start height for rescan
    pub async fn z_importviewingkey(
        &self,
        viewing_key: &str,
        rescan: Option<u32>,
        start_height: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut params = vec![serde_json::json!(viewing_key)];
        
        if let Some(height) = rescan {
            params.push(serde_json::json!(height));
        }
        
        if let Some(start) = start_height {
            params.push(serde_json::json!(start));
        }
        
        self.call("z_importviewingkey", params).await
    }
}