use anyhow::Result;
use zcash_address::unified::{Address, Encoding, Receiver};
use orchard::keys::{FullViewingKey, SpendingKey};

pub struct BridgeAddressGenerator;

impl BridgeAddressGenerator {
    /// Generate a valid Zcash Unified Address from FROST group key
    pub fn generate_bridge_ua(
        frost_group_key: &[u8],
        network: &str,
    ) -> Result<String> {
        // Determine network (use zcash_address::Network directly)
        let zcash_network = match network {
            "mainnet" => zcash_address::Network::Main,
            "testnet" => zcash_address::Network::Test,
            _ => anyhow::bail!("Invalid network: {}", network),
        };

        // Convert FROST key bytes to Orchard spending key
        let spending_key = Self::frost_key_to_orchard_sk(frost_group_key)?;
        
        // Derive full viewing key
        let fvk = FullViewingKey::from(&spending_key);
        
        // Get default address (diversifier index 0)
        let orchard_address = fvk.address_at(0u32, orchard::keys::Scope::External);
        
        // Build Unified Address with Orchard receiver
        let ua = Address::try_from_items(vec![
            Receiver::Orchard(orchard_address.to_raw_address_bytes()),
        ])?;
        
        // Encode to string
        Ok(ua.encode(&zcash_network))
    }

    /// Convert FROST group key to Orchard spending key
    fn frost_key_to_orchard_sk(frost_key: &[u8]) -> Result<SpendingKey> {
        if frost_key.len() != 32 {
            anyhow::bail!("FROST key must be 32 bytes, got {}", frost_key.len());
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&frost_key[..32]);

        // SpendingKey::from_bytes returns CtOption<SpendingKey>
        // Convert to Option using Into trait
        let sk_option: Option<SpendingKey> = SpendingKey::from_bytes(key_bytes).into();
        let sk = sk_option
            .ok_or_else(|| anyhow::anyhow!("Invalid key bytes for Orchard spending key"))?;

        Ok(sk)
    }

    /// Derive Full Viewing Key from FROST group key
    pub fn derive_fvk(frost_group_key: &[u8]) -> Result<FullViewingKey> {
        let sk = Self::frost_key_to_orchard_sk(frost_group_key)?;
        Ok(FullViewingKey::from(&sk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ua_generation() {
        // Mock FROST group key (32 bytes)
        let frost_key = [0x42u8; 32];
        
        // Generate testnet UA
        let ua = BridgeAddressGenerator::generate_bridge_ua(&frost_key, "testnet")
            .expect("Should generate valid UA");
        
        // Verify it starts with u1test (testnet unified address)
        assert!(ua.starts_with("u1test"), "Should be testnet UA, got: {}", ua);
        
        // Verify it's not a placeholder
        assert!(!ua.contains("PLACEHOLDER"), "Should not be placeholder");
        
        println!("Generated UA: {}", ua);
    }

    #[test]
    fn test_fvk_derivation() {
        let frost_key = [0x42u8; 32];
        let fvk = BridgeAddressGenerator::derive_fvk(&frost_key)
            .expect("Should derive FVK");
        
        // Verify we can get an address from it
        let address = fvk.address_at(0u32, orchard::keys::Scope::External);
        assert_eq!(address.to_raw_address_bytes().len(), 43);
    }
}