use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};

/// Simple in-memory network for testing (simulates P2P)
#[derive(Clone)]
pub struct NetworkClient {
    node_id: u16,
    storage: Arc<Mutex<NetworkStorage>>,
}

#[derive(Default)]
pub struct NetworkStorage {
    round1_messages: HashMap<u16, Vec<u8>>,
    round2_messages: HashMap<(u16, u16), Vec<u8>>,
    // For FROST signing
    commitments: HashMap<u16, Vec<u8>>,
    signature_shares: HashMap<u16, Vec<u8>>,
}

impl NetworkClient {
    pub fn new(node_id: u16, storage: Arc<Mutex<NetworkStorage>>) -> Self {
        Self { node_id, storage }
    }

    // DKG methods
    pub async fn broadcast_round1(&self, from: u16, data: Vec<u8>) -> Result<()> {
        let mut store = self.storage.lock().unwrap();
        store.round1_messages.insert(from, data);
        Ok(())
    }

    pub async fn receive_round1(&self, from: u16) -> Result<Vec<u8>> {
        for _ in 0..100 {
            {
                let store = self.storage.lock().unwrap();
                if let Some(data) = store.round1_messages.get(&from) {
                    return Ok(data.clone());
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
        anyhow::bail!("Timeout waiting for Round 1 from node {}", from)
    }

    pub async fn send_round2(&self, from: u16, to: u16, data: Vec<u8>) -> Result<()> {
        let mut store = self.storage.lock().unwrap();
        store.round2_messages.insert((from, to), data);
        Ok(())
    }

    pub async fn receive_round2(&self, from: u16, to: u16) -> Result<Vec<u8>> {
        for _ in 0..100 {
            {
                let store = self.storage.lock().unwrap();
                if let Some(data) = store.round2_messages.get(&(from, to)) {
                    return Ok(data.clone());
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
        anyhow::bail!("Timeout waiting for Round 2 from node {} to {}", from, to)
    }

    // FROST Signing methods
    pub async fn broadcast_commitments(&self, data: Vec<u8>) -> Result<()> {
        let mut store = self.storage.lock().unwrap();
        store.commitments.insert(self.node_id, data);
        Ok(())
    }

    pub async fn receive_commitments(&self, from: u16) -> Result<Vec<u8>> {
        for _ in 0..100 {
            {
                let store = self.storage.lock().unwrap();
                if let Some(data) = store.commitments.get(&from) {
                    return Ok(data.clone());
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("Timeout waiting for commitments from node {}", from)
    }

    pub async fn broadcast_signature_shares(&self, data: Vec<u8>) -> Result<()> {
        let mut store = self.storage.lock().unwrap();
        store.signature_shares.insert(self.node_id, data);
        Ok(())
    }

    pub async fn receive_signature_shares(&self, from: u16) -> Result<Vec<u8>> {
        for _ in 0..100 {
            {
                let store = self.storage.lock().unwrap();
                if let Some(data) = store.signature_shares.get(&from) {
                    return Ok(data.clone());
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("Timeout waiting for signature shares from node {}", from)
    }
}

pub fn create_shared_network() -> Arc<Mutex<NetworkStorage>> {
    Arc::new(Mutex::new(NetworkStorage::default()))
}