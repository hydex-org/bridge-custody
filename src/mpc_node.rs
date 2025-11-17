use anyhow::{Result, Context};
use tracing::{info, error};
use std::sync::{Arc, Mutex};

use crate::types::*;
use crate::dkg_coordinator::DkgCoordinator;
use crate::network::{NetworkClient, NetworkStorage};

pub struct MpcNode {
    config: NodeConfig,
    network: NetworkClient,
    dkg_result: Option<DkgResult>,
}

impl MpcNode {
    pub fn new(config: NodeConfig, network_storage: Arc<Mutex<NetworkStorage>>) -> Result<Self> {
        info!("Initializing MPC node {}", config.node_id);
        
        let network = NetworkClient::new(config.node_id, network_storage);
        
        Ok(Self { 
            config,
            network,
            dkg_result: None,
        })
    }
    
    pub async fn initialize_with_dkg(&mut self) -> Result<String> {
        info!("Starting DKG ceremony for node {}", self.config.node_id);
        
        let mut coordinator = DkgCoordinator::new(
            self.config.node_id,
            self.config.total_nodes,
            self.config.threshold,
            self.network.clone(), // TODO: Make NetworkClient cloneable
        );
        
        let dkg_result = coordinator.run_ceremony().await?;
        let bridge_ua = dkg_result.bridge_ua.clone();
        
        self.dkg_result = Some(dkg_result);
        
        Ok(bridge_ua)
    }
    
    pub fn load_existing_keys(&mut self) -> Result<()> {
        info!("Loading existing keys for node {}", self.config.node_id);
        todo!("Key loading implementation")
    }
    
    pub async fn run(&self) -> Result<()> {
        info!("MPC node {} running", self.config.node_id);
        
        // Keep running
        tokio::signal::ctrl_c().await?;
        info!("Shutting down...");
        
        Ok(())
    }
}