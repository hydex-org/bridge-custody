use anyhow::Result;
use frost_pallas::keys::dkg;
use frost_pallas::Identifier;
use std::collections::BTreeMap;
use tracing::{debug, info};
use rand::thread_rng;

use crate::network::NetworkClient;
use crate::network_http::HttpNetworkClient;
use crate::types::DkgResult;
use crate::ua_builder::BridgeAddressGenerator;

#[derive(Clone)]
pub enum NetworkBackend {
    InMemory(NetworkClient),
    Http(HttpNetworkClient),
}

pub struct DkgCoordinator {
    node_id: u16,
    total_nodes: u16,
    threshold: u16,
    network: NetworkBackend,
}

impl DkgCoordinator {
    pub fn new(
        node_id: u16,
        total_nodes: u16,
        threshold: u16,
        network: NetworkClient,
    ) -> Self {
        Self {
            node_id,
            total_nodes,
            threshold,
            network: NetworkBackend::InMemory(network),
        }
    }

    pub fn new_with_http(
        node_id: u16,
        total_nodes: u16,
        threshold: u16,
        client: HttpNetworkClient,
    ) -> Self {
        Self {
            node_id,
            total_nodes,
            threshold,
            network: NetworkBackend::Http(client),
        }
    }

    pub async fn run_ceremony(&mut self, zcash_network: &str) -> Result<DkgResult> {
        info!("Starting DKG ceremony for node {}", self.node_id);
        
        let my_id: Identifier = self.node_id.try_into()?;
        
        // Round 1: Generate and exchange commitments
        info!("Round 1: Generating commitments");
        let (round1_secret, round1_package) = self.dkg_round1(my_id)?;
        let round1_packages = self.exchange_round1_packages(my_id, round1_package).await?;
        
        // Round 2: Generate and exchange shares
        info!("Round 2: Generating shares");
        let (round2_secret, my_round2_packages) = self.dkg_round2(round1_secret, &round1_packages)?;
        let round2_packages = self.exchange_round2_packages(my_id, my_round2_packages).await?;
        
        // Round 3: Finalize key shares
        info!("Round 3: Finalizing key shares");
        let (_key_package, public_key_package) = self.dkg_round3(
            &round2_secret,
            &round1_packages,
            &round2_packages,
        )?;

        let group_vk = public_key_package.verifying_key();
        let vk_bytes = group_vk.serialize()?;
        
        info!("✓ Round 3 complete!");

        // Generate REAL Zcash Unified Address
        let bridge_ua = BridgeAddressGenerator::generate_bridge_ua(&vk_bytes, zcash_network)?;

        // Derive Orchard Full Viewing Key for Enclave
        let ufvk_encoded = BridgeAddressGenerator::derive_ufvk_encoded(&vk_bytes, zcash_network)?;

        info!("✅ DKG ceremony successful!");
        info!("   Group key: {}", hex::encode(&vk_bytes));
        info!("🎉 Bridge UA: {}", bridge_ua);
        info!("👁️  FVK (Orchard): {}", ufvk_encoded);

        Ok(DkgResult {
            node_id: self.node_id,
            key_share: vec![], // TODO: Serialize key_package
            group_verifying_key: vk_bytes,
            bridge_ua,
            full_viewing_key: ufvk_encoded,
        })
    }

    fn dkg_round1(
        &self,
        my_id: Identifier,
    ) -> Result<(dkg::round1::SecretPackage, dkg::round1::Package)> {
        let mut rng = thread_rng();
        
        let (secret, package) = dkg::part1(
            my_id,
            self.total_nodes,
            self.threshold,
            &mut rng,
        )?;
        
        debug!("Round 1: Generated commitment package");
        Ok((secret, package))
    }

    fn dkg_round2(
        &self,
        round1_secret: dkg::round1::SecretPackage,
        round1_packages: &BTreeMap<Identifier, dkg::round1::Package>,
    ) -> Result<(dkg::round2::SecretPackage, BTreeMap<Identifier, dkg::round2::Package>)> {
        let (secret, packages) = dkg::part2(round1_secret, round1_packages)?;
        
        debug!("Round 2: Generated {} share packages", packages.len());
        Ok((secret, packages))
    }

    fn dkg_round3(
        &self,
        round2_secret: &dkg::round2::SecretPackage,
        round1_packages: &BTreeMap<Identifier, dkg::round1::Package>,
        round2_packages: &BTreeMap<Identifier, dkg::round2::Package>,
    ) -> Result<(frost_pallas::keys::KeyPackage, frost_pallas::keys::PublicKeyPackage)> {
        let (_key_package, public_key_package) = dkg::part3(
            round2_secret,
            round1_packages,
            round2_packages,
        )?;
        
        debug!("Round 3: Key finalized");
        Ok((_key_package, public_key_package))
    }

    async fn exchange_round1_packages(
        &mut self,
        _my_id: Identifier,
        my_package: dkg::round1::Package,
    ) -> Result<BTreeMap<Identifier, dkg::round1::Package>> {
        let serialized = serde_json::to_vec(&my_package)?;
        
        match &self.network {
            NetworkBackend::InMemory(client) => {
                client.broadcast_round1(self.node_id, serialized).await?;
            }
            NetworkBackend::Http(client) => {
                client.broadcast_round1(serialized).await?;
            }
        }

        debug!("Node {} broadcast Round 1 package", self.node_id);
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let mut packages = BTreeMap::new();

        for node_id in 1..=self.total_nodes {
            if node_id == self.node_id {
                continue;
            }
            
            debug!("Node {} waiting for Round 1 from node {}", self.node_id, node_id);
            
            let data = match &self.network {
                NetworkBackend::InMemory(client) => {
                    client.receive_round1(node_id).await?
                }
                NetworkBackend::Http(client) => {
                    client.receive_round1(node_id).await?
                }
            };
            
            let package: dkg::round1::Package = serde_json::from_slice(&data)?;
            let id: Identifier = node_id.try_into()?;
            packages.insert(id, package);
            debug!("Node {} received Round 1 from node {}", self.node_id, node_id);
        }

        debug!("Node {} collected {} Round 1 packages from others", self.node_id, packages.len());
        Ok(packages)
    }

    async fn exchange_round2_packages(
        &mut self,
        _my_id: Identifier,
        my_packages: BTreeMap<Identifier, dkg::round2::Package>,
    ) -> Result<BTreeMap<Identifier, dkg::round2::Package>> {
        for (recipient_id, package) in &my_packages {
            let serialized = serde_json::to_vec(package)?;
            let recipient_node: u16 = {
                let bytes = recipient_id.serialize();
                u16::from_le_bytes([bytes[0], bytes[1]])
            };
            
            match &self.network {
                NetworkBackend::InMemory(client) => {
                    client.send_round2(self.node_id, recipient_node, serialized).await?;
                }
                NetworkBackend::Http(client) => {
                    client.send_round2(recipient_node, serialized).await?;
                }
            }
            
            debug!("Node {} sent Round 2 to node {}", self.node_id, recipient_node);
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let mut received_packages = BTreeMap::new();
        
        for node_id in 1..=self.total_nodes {
            if node_id == self.node_id {
                continue;
            }
            
            debug!("Node {} waiting for Round 2 from node {}", self.node_id, node_id);
            
            let data = match &self.network {
                NetworkBackend::InMemory(client) => {
                    client.receive_round2(node_id, self.node_id).await?
                }
                NetworkBackend::Http(client) => {
                    client.receive_round2(node_id, self.node_id).await?
                }
            };
            
            let package: dkg::round2::Package = serde_json::from_slice(&data)?;
            let id: Identifier = node_id.try_into()?;
            received_packages.insert(id, package);
            debug!("Node {} received Round 2 from node {}", self.node_id, node_id);
        }

        debug!("Node {} collected {} Round 2 packages from others", self.node_id, received_packages.len());
        Ok(received_packages)
    }
}