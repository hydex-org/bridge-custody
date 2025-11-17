use anyhow::Result;
use crate::types::WithdrawalRequest;

pub struct SolanaEventListener {
    rpc_url: String,
}

impl SolanaEventListener {
    pub fn new(rpc_url: String, program_id: String) -> Result<Self> {
        println!("Solana Listener: Connecting to {}", rpc_url);
        Ok(Self { rpc_url })
    }
    
    pub async fn listen_for_burns(&mut self) -> Result<Vec<WithdrawalRequest>> {
        // TODO: Poll Solana for burn events
        // TODO: Parse transaction logs
        // TODO: Return withdrawal requests
        
        todo!("Implement Solana event listening")
    }
}