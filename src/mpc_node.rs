use anyhow::Result;
use crate::types::NodeConfig;
use crate::network::NetworkClient;
use crate::dkg_coordinator::DkgCoordinator;
use std::sync::{Arc, Mutex};

pub struct MpcNode {
    config: NodeConfig,
    pub network: NetworkClient,  // Made public for tests
}

impl MpcNode {
    pub fn new(
        config: NodeConfig,
        network_storage: Arc<Mutex<crate::network::NetworkStorage>>,
    ) -> Result<Self> {
        let network = NetworkClient::new(config.node_id, network_storage);
        
        Ok(Self {
            config,
            network,
        })
    }

    pub async fn initialize_with_dkg(&mut self) -> Result<String> {
        let mut coordinator = DkgCoordinator::new(
            self.config.node_id,
            self.config.total_nodes,
            self.config.threshold,
            self.network.clone(),
        );

        let result = coordinator.run_ceremony(&self.config.zcash.network).await?;
        Ok(result.bridge_ua)
    }

    pub fn load_existing_keys(&mut self) -> Result<()> {
        todo!("Load persisted keys from disk")
    }

    pub async fn run(&self) -> Result<()> {
        todo!("Run node services (signing, API, etc.)")
    }
}