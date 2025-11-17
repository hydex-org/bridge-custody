use anyhow::{Context, Result};
use frost_pallas::{keys::dkg, Identifier};
use rand::thread_rng;
use std::collections::BTreeMap;
use tracing::{info, debug};

use crate::types::DkgResult;
use crate::network::NetworkClient;

pub struct DkgCoordinator {
    node_id: u16,
    total_nodes: u16,
    threshold: u16,
    network: NetworkClient,
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
            network,
        }
    }

    /// Run complete DKG ceremony (3 rounds)
    pub async fn run_ceremony(&mut self) -> Result<DkgResult> {
        info!("🔐 Starting DKG ceremony");
        info!("   Node {}/{} (threshold: {})", self.node_id, self.total_nodes, self.threshold);

        let my_id: Identifier = self.node_id.try_into()
            .context("Invalid node ID")?;

        // ROUND 1: Generate commitments
        info!("📋 Round 1: Generating commitments...");
        let (round1_secret, round1_package) = self.dkg_round1(my_id)?;
        
        // Broadcast to other nodes
        let round1_packages = self.exchange_round1_packages(my_id, round1_package).await?;
        info!("✓ Round 1 complete - received {} packages", round1_packages.len());

        // ROUND 2: Generate and distribute shares
        info!("📋 Round 2: Distributing secret shares...");
        let (round2_secret, round2_packages) = self.dkg_round2(round1_secret, &round1_packages)?;
        
        // Exchange round 2 packages
        let my_round2_packages = self.exchange_round2_packages(my_id, round2_packages).await?;
        info!("✓ Round 2 complete - received {} packages", my_round2_packages.len());

        // ROUND 3: Finalize keys
        info!("📋 Round 3: Finalizing keys...");
        let (key_package, public_key_package) = self.dkg_round3(
            &round2_secret,
            &round1_packages,
            &my_round2_packages,
        )?;

        let group_vk = public_key_package.verifying_key();
        let vk_bytes = group_vk.serialize()?;
        
        info!("✓ Round 3 complete!");
        info!("✅ DKG ceremony successful!");
        info!("   Group key: {}", hex::encode(&vk_bytes));

        // TODO: Generate proper UA from group key
        let bridge_ua = format!("u1test_PLACEHOLDER_{}", hex::encode(&vk_bytes[..8]));

        Ok(DkgResult {
            node_id: self.node_id,
            key_share: vec![], // TODO: Serialize key_package
            group_verifying_key: vk_bytes,
            bridge_ua,
        })
    }

    /// DKG Round 1: Generate commitments
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

    /// DKG Round 2: Process others' commitments and generate shares
    fn dkg_round2(
        &self,
        round1_secret: dkg::round1::SecretPackage,
        round1_packages: &BTreeMap<Identifier, dkg::round1::Package>,
    ) -> Result<(dkg::round2::SecretPackage, BTreeMap<Identifier, dkg::round2::Package>)> {
        let (secret, packages) = dkg::part2(round1_secret, round1_packages)?;
        
        debug!("Round 2: Generated {} share packages", packages.len());
        Ok((secret, packages))
    }

    /// DKG Round 3: Finalize with received shares
    fn dkg_round3(
        &self,
        round2_secret: &dkg::round2::SecretPackage,
        round1_packages: &BTreeMap<Identifier, dkg::round1::Package>,
        round2_packages: &BTreeMap<Identifier, dkg::round2::Package>,
    ) -> Result<(frost_pallas::keys::KeyPackage, frost_pallas::keys::PublicKeyPackage)> {
        let (key_package, public_key_package) = dkg::part3(
            round2_secret,
            round1_packages,
            round2_packages,
        )?;
        
        debug!("Round 3: Key finalized");
        Ok((key_package, public_key_package))
    }

    /// Exchange Round 1 packages with other nodes
    async fn exchange_round1_packages(
        &mut self,
        my_id: Identifier,
        my_package: dkg::round1::Package,
    ) -> Result<BTreeMap<Identifier, dkg::round1::Package>> {
        // Serialize and broadcast
        let serialized = serde_json::to_vec(&my_package)?;
        self.network.broadcast_round1(self.node_id, serialized).await?;

        // Collect from others
        let mut packages = BTreeMap::new();
        packages.insert(my_id, my_package); // Include our own

        for node_id in 1..=self.total_nodes {
            if node_id == self.node_id {
                continue;
            }
            
            let data = self.network.receive_round1(node_id).await?;
            let package: dkg::round1::Package = serde_json::from_slice(&data)?;
            let id: Identifier = node_id.try_into()?;
            packages.insert(id, package);
        }

        Ok(packages)
    }

    /// Exchange Round 2 packages with other nodes
    async fn exchange_round2_packages(
        &mut self,
        my_id: Identifier,
        my_packages: BTreeMap<Identifier, dkg::round2::Package>,
    ) -> Result<BTreeMap<Identifier, dkg::round2::Package>> {
        // Send each package to its intended recipient
        for (recipient_id, package) in &my_packages {
            let serialized = serde_json::to_vec(package)?;
            let recipient_node: u16 = (*recipient_id).into();
            self.network.send_round2(self.node_id, recipient_node, serialized).await?;
        }

        // Receive packages from others
        let mut received_packages = BTreeMap::new();
        
        for node_id in 1..=self.total_nodes {
            if node_id == self.node_id {
                continue;
            }
            
            let data = self.network.receive_round2(node_id, self.node_id).await?;
            let package: dkg::round2::Package = serde_json::from_slice(&data)?;
            let id: Identifier = node_id.try_into()?;
            received_packages.insert(id, package);
        }

        Ok(received_packages)
    }
}