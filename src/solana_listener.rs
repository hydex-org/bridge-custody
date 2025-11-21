use anyhow::Result;
use crate::types::WithdrawalRequest;

pub struct SolanaEventListener {
    _rpc_url: String,
    _program_id: String,  // Also prefix this to suppress warning
}

impl SolanaEventListener {
    pub fn new(rpc_url: String, program_id: String) -> Result<Self> {
        println!("Solana Listener: Connecting to {}", rpc_url);
        Ok(Self { 
            _rpc_url: rpc_url,      // ← FIX: Match the field name with underscore
            _program_id: program_id, // ← FIX: Also store program_id
        })
    }
    
    pub async fn listen_for_burns(&mut self) -> Result<Vec<WithdrawalRequest>> {
        // TODO: Poll Solana for burn events
        // TODO: Parse transaction logs
        // TODO: Return withdrawal requests
        
        todo!("Implement Solana event listening")
    }
}