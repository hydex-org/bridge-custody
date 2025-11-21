//! Simple lightwalletd HTTP client
//! 
//! Uses REST endpoints instead of gRPC to avoid protobuf complexity

use anyhow::{Result, bail};
use serde::Deserialize;
#[derive(Debug, Deserialize)]
pub struct BlockInfo {
    pub height: u64,
    pub hash: String,
    pub time: u64,
}

#[derive(Debug, Deserialize)]
pub struct TxInfo {
    pub txid: String,
    pub height: u64,
    pub confirmations: u64,
}

pub struct LightwalletdClient {
    client: reqwest::Client,
    base_url: String,
}

impl LightwalletdClient {
    pub fn new(server_url: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            base_url: server_url,
        })
    }

    pub async fn get_latest_block(&self) -> Result<u64> {
        // For now, we'll use a simpler approach
        // Real implementation would use gRPC CompactTxStreamer.GetLatestBlock
        
        // Return a reasonable testnet height for demonstration
        Ok(3_689_000)
    }

    pub async fn health_check(&self) -> Result<()> {
        let response = self.client
            .get(&self.base_url)
            .send()
            .await?;
        
        if response.status().is_success() {
            Ok(())
        } else {
            bail!("Server returned error: {}", response.status())
        }
    }
}