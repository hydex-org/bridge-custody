//! Real gRPC client for lightwalletd

use anyhow::Result;
use tonic::transport::Channel;
use tokio_stream::StreamExt;

// Include generated protobuf code
pub mod wallet {
    tonic::include_proto!("cash.z.wallet.sdk.rpc");
}

use wallet::{
    compact_tx_streamer_client::CompactTxStreamerClient,
    BlockId, BlockRange, ChainSpec, CompactBlock,
};

pub struct LightwalletdClient {
    client: CompactTxStreamerClient<Channel>,
}

impl LightwalletdClient {
    pub async fn connect(endpoint: String) -> Result<Self> {
        let client = CompactTxStreamerClient::connect(endpoint).await?;
        
        Ok(Self { client })
    }

    pub async fn get_latest_block(&mut self) -> Result<u64> {
        let response = self.client
            .get_latest_block(ChainSpec {})
            .await?;
        
        Ok(response.into_inner().height)
    }

    pub async fn get_block_range(
        &mut self,
        start: u64,
        end: u64,
    ) -> Result<Vec<CompactBlock>> {
        let request = BlockRange {
            start: Some(BlockId {
                height: start,
                hash: vec![],
            }),
            end: Some(BlockId {
                height: end,
                hash: vec![],
            }),
        };

        let mut stream = self.client
            .get_block_range(request)
            .await?
            .into_inner();

        let mut blocks = Vec::new();
        while let Some(block) = stream.next().await {
            blocks.push(block?);
        }

        Ok(blocks)
    }
}