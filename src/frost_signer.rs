use anyhow::Result;
use frost_pallas::{Identifier, SigningPackage, Signature};
use frost_pallas::round1::{SigningCommitments, SigningNonces};
use frost_pallas::round2::SignatureShare;
use std::collections::BTreeMap;
use tracing::{debug, info};
use rand::thread_rng;

use crate::network::NetworkClient;
use crate::network_http::HttpNetworkClient;

#[derive(Clone)]
pub enum NetworkBackend {
    InMemory(NetworkClient),
    Http(HttpNetworkClient),
}
/// Coordinates FROST threshold signing ceremony
pub struct FrostSigningCoordinator {
    node_id: u16,
    total_nodes: u16,
    threshold: u16,
    network: NetworkBackend,
}

impl FrostSigningCoordinator {
    /// Create new signing coordinator with in-memory network
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

    /// Create new signing coordinator with HTTP network
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

    /// Execute FROST signing ceremony
    pub async fn sign_message(
        &mut self,
        message: &[u8],
        key_package: &frost_pallas::keys::KeyPackage,
        pub_key_pkg: &frost_pallas::keys::PublicKeyPackage,
    ) -> Result<Signature> {
        info!("🖊️  Starting FROST signing ceremony for node {}", self.node_id);
        
        let my_id: Identifier = self.node_id.try_into()?;
        
        // Round 1: Generate and exchange signing commitments
        info!("Round 1: Generating signing commitments");
        let (nonces, commitments) = self.signing_round1(key_package)?;
        let all_commitments = self.exchange_round1_commitments(my_id, commitments).await?;
        
        // Round 2: Generate and exchange signature shares
        info!("Round 2: Generating signature shares");
        let signature_share = self.signing_round2(
            message,
            key_package,
            &nonces,
            &all_commitments,
        )?;
        let all_shares = self.exchange_round2_shares(my_id, signature_share).await?;
        
        // Aggregate: Combine signature shares into final signature
        info!("Aggregating signature shares");
        let final_signature = self.aggregate_signatures(
            message,
            &all_commitments,
            &all_shares,
            pub_key_pkg,
        )?;
        
        info!("✅ FROST signing complete!");
        info!("   Signature: {}", hex::encode(final_signature.serialize()?));
        
        Ok(final_signature)
    }

    /// Round 1: Generate signing nonces and commitments
    fn signing_round1(
        &self,
        key_package: &frost_pallas::keys::KeyPackage,
    ) -> Result<(SigningNonces, SigningCommitments)> {
        let mut rng = thread_rng();
        
        // Generate nonces with the secret key share
        let nonces = SigningNonces::new(key_package.signing_share(), &mut rng);
        
        // Get commitments (returns a reference, so we dereference)
        let commitments = *nonces.commitments();
        
        debug!("Generated signing commitments for node {}", self.node_id);
        
        Ok((nonces, commitments))
    }

    /// Exchange Round 1 commitments with all participants
    async fn exchange_round1_commitments(
        &self,
        my_id: Identifier,
        my_commitments: SigningCommitments,
    ) -> Result<BTreeMap<Identifier, SigningCommitments>> {
        let mut all_commitments = BTreeMap::new();
        
        // Serialize our commitments
        let my_data = bincode::serialize(&my_commitments)?;
        
        match &self.network {
            NetworkBackend::InMemory(client) => {
                // Broadcast our commitments
                client.broadcast_commitments(my_data.clone()).await?;
                
                // Collect from all OTHER nodes
                for node_id in 1..=self.total_nodes {
                    if node_id != self.node_id {
                        let data = client.receive_commitments(node_id).await?;
                        let commitments: SigningCommitments = bincode::deserialize(&data)?;
                        let id: Identifier = node_id.try_into()?;
                        all_commitments.insert(id, commitments);
                    }
                }
            }
            NetworkBackend::Http(client) => {
                // Broadcast to all peers
                client.broadcast_signing_commitments(my_data).await?;
                
                // Wait for responses from all OTHER nodes
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                
                for node_id in 1..=self.total_nodes {
                    if node_id != self.node_id {
                        let data = client.receive_signing_commitments(node_id).await?;
                        let commitments: SigningCommitments = bincode::deserialize(&data)?;
                        let id: Identifier = node_id.try_into()?;
                        all_commitments.insert(id, commitments);
                    }
                }
            }
        }
        
        // Add our own commitments
        all_commitments.insert(my_id, my_commitments);
        
        info!("Collected {} commitments", all_commitments.len());
        
        Ok(all_commitments)
    }

    /// Round 2: Generate signature share
    fn signing_round2(
        &self,
        message: &[u8],
        key_package: &frost_pallas::keys::KeyPackage,
        nonces: &SigningNonces,
        commitments: &BTreeMap<Identifier, SigningCommitments>,
    ) -> Result<SignatureShare> {
        // Create signing package (message + commitments)
        let signing_package = SigningPackage::new(commitments.clone(), message);
        
        // Generate our signature share
        let signature_share = frost_pallas::round2::sign(
            &signing_package,
            nonces,
            key_package,
        )?;
        
        debug!("Generated signature share for node {}", self.node_id);
        
        Ok(signature_share)
    }

    /// Exchange Round 2 signature shares
    async fn exchange_round2_shares(
        &self,
        my_id: Identifier,
        my_share: SignatureShare,
    ) -> Result<BTreeMap<Identifier, SignatureShare>> {
        let mut all_shares = BTreeMap::new();
        
        // Serialize our share
        let my_data = bincode::serialize(&my_share)?;
        
        match &self.network {
            NetworkBackend::InMemory(client) => {
                // Broadcast our share
                client.broadcast_signature_shares(my_data.clone()).await?;
                
                // Collect from all OTHER nodes
                for node_id in 1..=self.total_nodes {
                    if node_id != self.node_id {
                        let data = client.receive_signature_shares(node_id).await?;
                        let share: SignatureShare = bincode::deserialize(&data)?;
                        let id: Identifier = node_id.try_into()?;
                        all_shares.insert(id, share);
                    }
                }
            }
            NetworkBackend::Http(client) => {
                // Broadcast to all peers
                client.broadcast_signature_shares(my_data).await?;
                
                // Wait for responses
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                
                for node_id in 1..=self.total_nodes {
                    if node_id != self.node_id {
                        let data = client.receive_signature_shares(node_id).await?;
                        let share: SignatureShare = bincode::deserialize(&data)?;
                        let id: Identifier = node_id.try_into()?;
                        all_shares.insert(id, share);
                    }
                }
            }
        }
        
        // Add our own share
        all_shares.insert(my_id, my_share);
        
        info!("Collected {} signature shares", all_shares.len());
        
        Ok(all_shares)
    }

    /// Aggregate signature shares into final signature
    fn aggregate_signatures(
        &self,
        message: &[u8],
        commitments: &BTreeMap<Identifier, SigningCommitments>,
        shares: &BTreeMap<Identifier, SignatureShare>,
        pub_key_pkg: &frost_pallas::keys::PublicKeyPackage,
    ) -> Result<Signature> {
        // Recreate signing package
        let signing_package = SigningPackage::new(commitments.clone(), message);
        
        // Aggregate shares
        let group_signature = frost_pallas::aggregate(
            &signing_package,
            shares,
            pub_key_pkg,
        )?;
        
        Ok(group_signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{create_shared_network, NetworkClient};
    use frost_pallas::keys::dkg;
    
    #[tokio::test]
    async fn test_three_node_frost_signing() {
        println!("\n🧪 Testing 3-Node FROST Signing");
        println!("================================");
        
        let total_nodes = 3u16;
        let threshold = 2u16;
        
        // Step 1: Run DKG to get key packages
        println!("📋 Step 1: Running DKG to generate keys...");
        let (key_packages, pub_key_pkg) = run_dkg_for_test(total_nodes, threshold).await;
        
        // Step 2: Create signing coordinators
        println!("📋 Step 2: Creating signing coordinators...");
        let storage = create_shared_network();
        let mut coordinators = Vec::new();
        
        for node_id in 1..=total_nodes {
            let client = NetworkClient::new(node_id, storage.clone());
            let coordinator = FrostSigningCoordinator::new(
                node_id,
                total_nodes,
                threshold,
                client,
            );
            coordinators.push(coordinator);
        }
        
        // Step 3: Sign a message
        let message = b"Test transaction hash for FROST signing";
        println!("📋 Step 3: Signing message: {:?}", String::from_utf8_lossy(message));
        
        let mut signatures = Vec::new();
        let mut handles = Vec::new();
        
        for (i, mut coordinator) in coordinators.into_iter().enumerate() {
            let key_pkg = key_packages[i].clone();
            let pub_pkg = pub_key_pkg.clone();
            let msg = message.to_vec();
            
            let handle = tokio::spawn(async move {
                coordinator.sign_message(&msg, &key_pkg, &pub_pkg).await
            });
            
            handles.push(handle);
        }
        
        for handle in handles {
            match handle.await {
                Ok(Ok(sig)) => {
                    println!("✓ Node generated signature");
                    signatures.push(sig);
                }
                Ok(Err(e)) => panic!("❌ Signing failed: {}", e),
                Err(e) => panic!("❌ Task failed: {}", e),
            }
        }
        
        // Step 4: Verify all nodes produced the same signature
        println!("📋 Step 4: Verifying signatures...");
        assert_eq!(signatures.len(), total_nodes as usize);
        
        let first_sig = &signatures[0];
        for sig in &signatures[1..] {
            assert_eq!(
                first_sig.serialize().unwrap(),
                sig.serialize().unwrap(),
                "All nodes should produce identical signatures"
            );
        }
        
        println!("✅ All nodes produced identical signatures!");
        println!("   Signature: {}", hex::encode(first_sig.serialize().unwrap()));
        
        // Step 5: Verify signature is valid
        let verifying_key = pub_key_pkg.verifying_key();
        let is_valid = verifying_key.verify(message, first_sig).is_ok();
        assert!(is_valid, "Signature should be valid");
        
        println!("✅ Signature verification passed!");
        println!("🎉 FROST signing test complete!\n");
    }
    
        // Helper: Run DKG to generate key packages for testing
        async fn run_dkg_for_test(
            total_nodes: u16,
            threshold: u16,
        ) -> (Vec<frost_pallas::keys::KeyPackage>, frost_pallas::keys::PublicKeyPackage) {
            use rand::thread_rng;
            
            let mut rng = thread_rng();
            let mut key_packages = Vec::new();
            let mut pub_key_package = None;
            
            let max_signers = total_nodes;
            let min_signers = threshold;
            
            let mut round1_packages = BTreeMap::new();
            let mut round1_secrets = BTreeMap::new();
            
            // Round 1
            for id in 1..=max_signers {
                let identifier: Identifier = id.try_into().unwrap();
                let (secret, package) = dkg::part1(identifier, max_signers, min_signers, &mut rng).unwrap();
                round1_secrets.insert(identifier, secret);
                round1_packages.insert(identifier, package);
            }
            
            // Round 2
            let mut round2_packages = BTreeMap::new();
            let mut round2_secrets = BTreeMap::new();
            
            for id in 1..=max_signers {
                let identifier: Identifier = id.try_into().unwrap();
                
                // Filter out THIS node's own package - part2 expects OTHER nodes' packages only
                let others_round1: BTreeMap<_, _> = round1_packages
                    .iter()
                    .filter(|(k, _)| **k != identifier)
                    .map(|(k, v)| (*k, v.clone()))
                    .collect();
                
                let (secret, packages) = dkg::part2(
                    round1_secrets.remove(&identifier).unwrap(),
                    &others_round1,  // ← Changed: only OTHER nodes' packages
                ).unwrap();
                round2_secrets.insert(identifier, secret);
                round2_packages.insert(identifier, packages);
            }
            
            // Round 3
            for id in 1..=max_signers {
                let identifier: Identifier = id.try_into().unwrap();
                
                // Collect Round 2 packages sent TO this node FROM other nodes
                let mut round2_for_this_node = BTreeMap::new();
                for (sender_id, packages) in &round2_packages {
                    if let Some(pkg) = packages.get(&identifier) {
                        round2_for_this_node.insert(*sender_id, pkg.clone());
                    }
                }
                
                // Filter out THIS node's own Round 1 package for part3
                let others_round1: BTreeMap<_, _> = round1_packages
                    .iter()
                    .filter(|(k, _)| **k != identifier)
                    .map(|(k, v)| (*k, v.clone()))
                    .collect();
                
                let (key_pkg, pub_pkg) = dkg::part3(
                    &round2_secrets[&identifier],
                    &others_round1,  // ← Changed: only OTHER nodes' packages
                    &round2_for_this_node,
                ).unwrap();
                
                key_packages.push(key_pkg);
                pub_key_package = Some(pub_pkg);
            }
            
            (key_packages, pub_key_package.unwrap())
        }
}