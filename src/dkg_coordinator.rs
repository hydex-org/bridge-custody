use anyhow::Result;
use tracing::info;
use crate::types::{DkgResult, FvkContribution};
use crate::network_http::HttpNetworkClient;
use crate::orchard_frost;
use crate::ua_builder::BridgeAddressGenerator;

pub struct DkgCoordinator {
    node_id: u16,
    total_nodes: u16,
    threshold: u16,
    network: HttpNetworkClient,
}

impl DkgCoordinator {
    pub fn new(
        node_id: u16,
        total_nodes: u16,
        threshold: u16,
        network: HttpNetworkClient,
    ) -> Self {
        Self {
            node_id,
            total_nodes,
            threshold,
            network,
        }
    }

    pub async fn run_ceremony(&mut self, network: &str) -> Result<DkgResult> {
        info!("Starting extended DKG ceremony for node {}", self.node_id);
        
        // Phase 1: Run FROST DKG
        info!("Phase 1: FROST DKG");
        let (key_package, public_key_package) = self.run_frost_dkg().await?;
        
        let vk_bytes = public_key_package.verifying_key().serialize()
            .map_err(|e| anyhow::anyhow!("Failed to serialize verifying key: {:?}", e))?;
        
        info!("✓ FROST DKG complete!");
        
        // Phase 2: Derive Orchard key shards from FROST secret
        info!("Phase 2: Deriving Orchard key shards");
        let signing_share = key_package.signing_share();
        let share_bytes = frost_pallas::keys::SigningShare::serialize(signing_share);
        
        let orchard_shards = orchard_frost::derive_orchard_shards_from_frost(
            &share_bytes,
            &vk_bytes,
            self.node_id,
        )?;
        
        // Phase 3: Compute FVK contribution
        info!("Phase 3: Computing FVK contributions");
        let fvk_contribution = orchard_frost::compute_fvk_contribution(&orchard_shards)?;
        
        // Phase 4: Exchange FVK contributions
        info!("Phase 4: Exchanging FVK contributions");
        let fvk_contributions = self.exchange_fvk_contributions(fvk_contribution).await?;
        
        // Phase 5: Aggregate FVK contributions
        info!("Phase 5: Aggregating Full Viewing Key");
        let aggregated_fvk = orchard_frost::aggregate_fvk_contributions(
            &fvk_contributions,
            &orchard_shards.rivk,
        )?;
        
        // CRITICAL: Calculate and store aggregated ak/nk for child derivation
        let (aggregated_ak_bytes, aggregated_nk_bytes) = {
            use pasta_curves::pallas;
            
            // Helper to parse point
            let parse_point = |bytes: &[u8]| -> Result<pallas::Point> {
                if bytes.len() != 32 {
                    anyhow::bail!("Expected 32 bytes");
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                Option::from(pallas::Point::from_bytes(&arr))
                    .ok_or_else(|| anyhow::anyhow!("Invalid point"))
            };
            
            // Sum all ak contributions
            let mut ak_sum = parse_point(&fvk_contributions[0].ak_bytes)?;
            for contrib in &fvk_contributions[1..] {
                let ak_i = parse_point(&contrib.ak_bytes)?;
                ak_sum = ak_sum + ak_i;
            }
            
            // Sum all nk contributions
            let mut nk_sum = parse_point(&fvk_contributions[0].nk_bytes)?;
            for contrib in &fvk_contributions[1..] {
                let nk_i = parse_point(&contrib.nk_bytes)?;
                nk_sum = nk_sum + nk_i;
            }
            
            use group::GroupEncoding;
            (ak_sum.to_bytes().to_vec(), nk_sum.to_bytes().to_vec())
        };
        
        // Phase 6: Generate bridge address
        info!("Phase 6: Generating bridge address");
        let bridge_ua = self.generate_ua_from_fvk(&aggregated_fvk, network)?;
        let ufvk_encoded = self.encode_ufvk_from_fvk(&aggregated_fvk, network)?;
        
        info!("✅ DKG ceremony successful!");
        info!("   Group key: {}", hex::encode(&vk_bytes));
        info!("🎉 Bridge UA: {}", bridge_ua);
        info!("👁️  UFVK: {}", ufvk_encoded);

        let shared_rivk = orchard_shards.rivk.clone();

        Ok(DkgResult {
            node_id: self.node_id,
            key_share: vec![],
            group_verifying_key: vk_bytes,
            orchard_shards,
            bridge_ua,
            full_viewing_key: ufvk_encoded,
            aggregated_ak: aggregated_ak_bytes,
            aggregated_nk: aggregated_nk_bytes,
            shared_rivk,
        })
    }

    async fn run_frost_dkg(&mut self) -> Result<(frost_pallas::keys::KeyPackage, frost_pallas::keys::PublicKeyPackage)> {
        use frost_pallas::{Identifier, keys::dkg};
        
        let id = Identifier::try_from(self.node_id)
            .map_err(|e| anyhow::anyhow!("Invalid identifier: {:?}", e))?;
        
        let max_signers = self.total_nodes;
        let min_signers = self.threshold;
        
        info!("Round 1: Generating secret polynomial (node {})", self.node_id);
        let (round1_secret, round1_package) = dkg::part1(
            id,
            max_signers,
            min_signers,
            &mut rand::thread_rng(),
        ).map_err(|e| anyhow::anyhow!("DKG round 1 failed: {:?}", e))?;
        
        let round1_bytes = round1_package.serialize()
            .map_err(|e| anyhow::anyhow!("Failed to serialize round1: {:?}", e))?;
        self.network.broadcast_round1(round1_bytes).await?;
        
        // Collect round1 packages from all nodes (including ourselves)
        let mut round1_packages = std::collections::BTreeMap::new();
        for node_id in 1..=self.total_nodes {
            let bytes = self.network.receive_round1(node_id).await?;
            let pkg = dkg::round1::Package::deserialize(&bytes)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize round1: {:?}", e))?;
            let sender_id = Identifier::try_from(node_id)
                .map_err(|e| anyhow::anyhow!("Invalid node_id {}: {:?}", node_id, e))?;
            round1_packages.insert(sender_id, pkg);
        }
        
        info!("Collected {} round1 packages (expected {})", round1_packages.len(), max_signers);
        
        // DEBUG: Show all identifiers in round1_packages
        info!("Round1 package identifiers:");
        for (id, _pkg) in &round1_packages {
            let id_bytes = id.serialize();
            let id_u16 = u16::from_le_bytes([id_bytes[0], id_bytes[1]]);
            info!("  - Identifier {:?} (node {})", id, id_u16);
        }

        // IMPORTANT: FROST part2 expects packages from OTHER participants only, not including our own
        // Remove our own package before calling part2
        let our_identifier = Identifier::try_from(self.node_id)
            .map_err(|e| anyhow::anyhow!("Invalid node_id: {:?}", e))?;
        round1_packages.remove(&our_identifier);
        info!("Removed our own round1 package (node {}). Now have {} packages for part2", self.node_id, round1_packages.len());

        info!("Round 2: Computing secret shares (node {})", self.node_id);
        info!("Calling part2 with {} round1 packages from OTHER nodes", round1_packages.len());
        let (round2_secret, round2_packages_generated) = dkg::part2(round1_secret, &round1_packages)
            .map_err(|e| anyhow::anyhow!("DKG round 2 failed: {:?}", e))?;
        info!("Part2 generated {} round2 packages (for other nodes)", round2_packages_generated.len());
        
        // DEBUG: Show who we're sending to
        info!("Round2 package recipients:");
        for (recipient_id, _) in &round2_packages_generated {
            let id_bytes = recipient_id.serialize();
            let recipient_node_id = u16::from_le_bytes([id_bytes[0], id_bytes[1]]);
            info!("  - Will send to node {}", recipient_node_id);
        }
        
        // Note: FROST part2 only generates packages for OTHER participants (n-1 packages)
        // We don't need our own Round 2 package
        
        // Send packages to other nodes
        for (recipient_id, round2_package) in round2_packages_generated {
            let round2_bytes = round2_package.serialize()
                .map_err(|e| anyhow::anyhow!("Failed to serialize round2: {:?}", e))?;
            // Convert Identifier to u16
            let id_bytes = recipient_id.serialize();
            let recipient_node_id = u16::from_le_bytes([id_bytes[0], id_bytes[1]]);
            self.network.send_round2(recipient_node_id, round2_bytes).await?;
        }
        
        // Collect round2 packages from other nodes (n-1 packages)
        let mut round2_packages = std::collections::BTreeMap::new();
        info!("Waiting for round2 packages from other nodes...");
        for node_id in 1..=self.total_nodes {
            if node_id == self.node_id {
                info!("  - Skipping node {} (ourselves)", node_id);
                continue; // Skip ourselves - we don't need our own package
            }
            info!("  - Waiting for round2 package from node {}...", node_id);
            let bytes = self.network.receive_round2(node_id).await?;
            let pkg = dkg::round2::Package::deserialize(&bytes)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize round2: {:?}", e))?;
            let sender_id = Identifier::try_from(node_id)
                .map_err(|e| anyhow::anyhow!("Invalid node_id {}: {:?}", node_id, e))?;
            round2_packages.insert(sender_id, pkg);
            info!("  ✓ Received round2 package from node {}", node_id);
        }
        info!("Collected {} round2 packages from other nodes", round2_packages.len());
        
        // DEBUG: Show what we have in round2_packages for part3
        info!("Round2 packages ready for part3:");
        for (id, _pkg) in &round2_packages {
            let id_bytes = id.serialize();
            let id_u16 = u16::from_le_bytes([id_bytes[0], id_bytes[1]]);
            info!("  - Package from node {}", id_u16);
        }
        
        info!("Round 3: Finalizing key shares");
        let (key_package, public_key_package) = dkg::part3(
            &round2_secret,
            &round1_packages,
            &round2_packages,
        ).map_err(|e| anyhow::anyhow!("DKG round 3 failed: {:?}", e))?;
        
        info!("✓ Round 3 complete!");
        
        Ok((key_package, public_key_package))
    }

    async fn exchange_fvk_contributions(&mut self, our_contribution: FvkContribution) -> Result<Vec<FvkContribution>> {
        info!("Exchanging FVK contributions with peers");
        
        // Serialize our contribution
        let our_bytes = serde_json::to_vec(&our_contribution)?;
        
        // Broadcast to all peers
        self.network.broadcast_fvk_contribution(&our_bytes).await?;
        
        // Collect from all nodes (including ourselves)
        let mut contributions = Vec::new();
        for node_id in 1..=self.total_nodes {
            let bytes = self.network.receive_fvk_contribution(node_id).await?;
            let contrib: FvkContribution = serde_json::from_slice(&bytes)?;
            contributions.push(contrib);
        }
        
        // Sort by node_id for deterministic ordering
        contributions.sort_by_key(|c| c.node_id);
        
        info!("✓ Collected {} FVK contributions", contributions.len());
        
        Ok(contributions)
    }

    fn generate_ua_from_fvk(&self, fvk: &orchard::keys::FullViewingKey, network: &str) -> Result<String> {
        let zcash_network = match network {
            "testnet" => zcash_primitives::consensus::Network::TestNetwork,
            "mainnet" => zcash_primitives::consensus::Network::MainNetwork,
            _ => anyhow::bail!("Invalid network: {}", network),
        };
        
        BridgeAddressGenerator::generate_address_from_fvk(fvk, zcash_network)
    }

    fn encode_ufvk_from_fvk(&self, fvk: &orchard::keys::FullViewingKey, network: &str) -> Result<String> {
        let zcash_network = match network {
            "testnet" => zcash_primitives::consensus::Network::TestNetwork,
            "mainnet" => zcash_primitives::consensus::Network::MainNetwork,
            _ => anyhow::bail!("Invalid network: {}", network),
        };
        
        BridgeAddressGenerator::encode_ufvk(fvk, zcash_network)
    }
}
