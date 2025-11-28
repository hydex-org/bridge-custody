/// Quick script to generate a test UFVK for API testing
/// Run with: cargo run --bin create_test_ufvk

use bridge_custody::ua_builder::BridgeAddressGenerator;
use bridge_custody::types::DkgResult;
use orchard::keys::{SpendingKey, FullViewingKey};

fn main() -> anyhow::Result<()> {
    println!("🔐 Generating test UFVK for API testing...\n");
    
    // Generate a test spending key (INSECURE - for testing only!)
    let test_seed = [42u8; 32];
    let sk_option: Option<SpendingKey> = SpendingKey::from_bytes(test_seed).into();
    let sk = sk_option.ok_or_else(|| anyhow::anyhow!("Invalid SK"))?;
    
    let fvk = FullViewingKey::from(&sk);
    let network = zcash_primitives::consensus::Network::TestNetwork;
    
    // Generate UA
    let bridge_ua = BridgeAddressGenerator::generate_address_from_fvk(&fvk, network)?;
    
    // Encode UFVK
    let ufvk = BridgeAddressGenerator::encode_ufvk(&fvk, network)?;
    
    println!("✅ Generated test keys:");
    println!("   Bridge UA: {}", bridge_ua);
    println!("   UFVK: {}\n", ufvk);
    
    // Create a minimal DKG result for each node
    let fvk_bytes = fvk.to_bytes();
    
    // Ensure data directories exist
    for node_id in 1..=3 {
        std::fs::create_dir_all(format!("data/node{}", node_id))?;
    }
    
    for node_id in 1..=3 {
        let result = DkgResult {
            node_id,
            key_share: vec![],
            group_verifying_key: vec![1, 2, 3], // Dummy
            orchard_shards: bridge_custody::types::OrchardKeyShards {
                ask_share: vec![0; 32],
                nsk_share: vec![0; 32],
                rivk: vec![0; 32],
                node_id,
            },
            bridge_ua: bridge_ua.clone(),
            full_viewing_key: ufvk.clone(),
            aggregated_ak: fvk_bytes[0..32].to_vec(),
            aggregated_nk: fvk_bytes[32..64].to_vec(),
            shared_rivk: fvk_bytes[64..96].to_vec(),
        };
        
        let path = format!("data/node{}/node{}_dkg_result.json", node_id, node_id);
        let json = serde_json::to_string_pretty(&result)?;
        std::fs::write(&path, json)?;
        println!("💾 Saved to {}", path);
    }
    
    println!("\n✅ Test UFVK setup complete!");
    println!("📝 You can now test the API with:");
    println!("   curl http://localhost:3001/api/bridge-address | python3 -m json.tool");
    println!("   curl -X POST http://localhost:3001/api/deposit-address \\");
    println!("     -H 'Content-Type: application/json' \\");
    println!("     -d '{{\"solana_pubkey\": \"alice\"}}' | python3 -m json.tool");
    
    Ok(())
}

