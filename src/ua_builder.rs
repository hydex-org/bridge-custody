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
    /// 
    /// ⚠️ SECURITY WARNING: This is INSECURE for production!
    /// The spending key is derived from the PUBLIC group verifying key,
    /// making funds theoretically vulnerable. This is acceptable for testnet
    /// development but MUST be replaced with proper key derivation for mainnet.
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

    /// Derive Orchard Full Viewing Key for Arcium Enclave
    /// 
    /// Returns the Orchard FVK encoded as hex with network prefix.
    /// This can be used by the Arcium Enclave for scanning Zcash blocks.
    /// 
    /// Note: For full ZIP 316 UFVK compliance (uviewtest... format),
    /// we would need to implement manual F4Jumble + Bech32m encoding
    /// or use a newer version of zcash_keys with better API support.
    pub fn derive_ufvk_encoded(
        frost_group_key: &[u8],
        network: &str,
    ) -> Result<String> {
        // Get Orchard FVK
        let sk = Self::frost_key_to_orchard_sk(frost_group_key)?;
        let orchard_fvk = FullViewingKey::from(&sk);
        
        // Encode FVK bytes as hex
        let fvk_bytes = orchard_fvk.to_bytes();
        
        // Add network prefix for clarity
        let prefix = match network {
            "mainnet" => "orchard-fvk-main",
            "testnet" => "orchard-fvk-test",
            _ => anyhow::bail!("Invalid network: {}", network),
        };
        
        // Return hex-encoded FVK with prefix
        // Format: "orchard-fvk-test:hexbytes..."
        // The Enclave can parse this and use the hex bytes for scanning
        Ok(format!("{}:{}", prefix, hex::encode(&fvk_bytes)))
    }

    /// Derive raw Orchard Full Viewing Key (for internal use)
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
        
        // Verify it starts with utest1 (testnet unified address)
        assert!(ua.starts_with("utest1"), "Should be testnet UA, got: {}", ua);
        
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

    #[test]
    fn test_ufvk_encoding() {
        let frost_key = [0x42u8; 32];
        
        // Test testnet FVK encoding
        let fvk_testnet = BridgeAddressGenerator::derive_ufvk_encoded(&frost_key, "testnet")
            .expect("Should derive testnet FVK");
        
        // Verify it has the correct prefix
        assert!(
            fvk_testnet.starts_with("orchard-fvk-test:"),
            "Testnet FVK should start with 'orchard-fvk-test:', got: {}",
            fvk_testnet
        );
        
        println!("Testnet FVK: {}", fvk_testnet);
        
        // Verify hex portion is valid
        let hex_part = fvk_testnet.split(':').nth(1).unwrap();
        assert!(
            hex_part.len() > 0 && hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "FVK hex should be valid hex"
        );
    }
}