use anyhow::Result;
use zcash_address::unified::{Address, Encoding, Receiver, Container};
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
    pub fn frost_key_to_orchard_sk(frost_key: &[u8]) -> Result<SpendingKey> {
        if frost_key.len() != 32 {
            anyhow::bail!("FROST key must be 32 bytes, got {}", frost_key.len());
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&frost_key[..32]);

        let sk_option: Option<SpendingKey> = SpendingKey::from_bytes(key_bytes).into();
        let sk = sk_option
            .ok_or_else(|| anyhow::anyhow!("Invalid key bytes for Orchard spending key"))?;

        Ok(sk)
    }

    /// Derive ZIP 316-compliant Unified Full Viewing Key (UFVK)
    pub fn derive_ufvk_encoded(
        frost_group_key: &[u8],
        network: &str,
    ) -> Result<String> {
        use zcash_address::{
            unified::{self, Encoding},
            Network,
        };
        
        // Get Orchard FVK
        let sk = Self::frost_key_to_orchard_sk(frost_group_key)?;
        let orchard_fvk = FullViewingKey::from(&sk);
        
        // Get FVK bytes
        let fvk_bytes = orchard_fvk.to_bytes();
        
        // Determine network
        let zcash_network = match network {
            "mainnet" => Network::Main,
            "testnet" => Network::Test,
            _ => anyhow::bail!("Invalid network: {}", network),
        };
        
        // Create a unified container with the Orchard FVK
        let items = vec![unified::Fvk::Orchard(fvk_bytes)];
        let ufvk = unified::Ufvk::try_from_items(items)
            .map_err(|e| anyhow::anyhow!("Failed to create UFVK: {:?}", e))?;
        
        // Encode it
        let encoded = ufvk.encode(&zcash_network);
        
        Ok(encoded)
    }

    /// Derive raw Orchard Full Viewing Key (for internal use)
    pub fn derive_fvk(frost_group_key: &[u8]) -> Result<FullViewingKey> {
        let sk = Self::frost_key_to_orchard_sk(frost_group_key)?;
        Ok(FullViewingKey::from(&sk))
    }

    /// Generate UA from an existing UFVK (for child address generation)
    pub fn generate_address_from_ufvk(
        ufvk_str: &str,
        network: zcash_primitives::consensus::Network,
    ) -> Result<String> {
        use zcash_address::{unified, Network};
        
        // Convert network types
        let zcash_network = match network {
            zcash_primitives::consensus::Network::MainNetwork => Network::Main,
            zcash_primitives::consensus::Network::TestNetwork => Network::Test,
        };
        
        // Decode UFVK
        let (decoded_net, ufvk) = unified::Ufvk::decode(ufvk_str)
            .map_err(|e| anyhow::anyhow!("Failed to decode UFVK: {:?}", e))?;
        
        if decoded_net != zcash_network {
            anyhow::bail!("Network mismatch");
        }
        
        // Extract Orchard FVK
        let mut orchard_fvk_bytes: Option<[u8; 96]> = None;
        for item in ufvk.items() {
            if let unified::Fvk::Orchard(bytes) = item {
                if bytes.len() == 96 {
                    let mut arr = [0u8; 96];
                    arr.copy_from_slice(&bytes);
                    orchard_fvk_bytes = Some(arr);
                    break;
                }
            }
        }
        
        let fvk_bytes = orchard_fvk_bytes
            .ok_or_else(|| anyhow::anyhow!("No Orchard FVK in UFVK"))?;
        
        // Parse FVK and get address
        let fvk = FullViewingKey::from_bytes(&fvk_bytes)
            .ok_or_else(|| anyhow::anyhow!("Invalid FVK bytes"))?;
        
        let orchard_address = fvk.address_at(0u32, orchard::keys::Scope::External);
        
        // Build Unified Address
        let ua = Address::try_from_items(vec![
            Receiver::Orchard(orchard_address.to_raw_address_bytes()),
        ])?;
        
        Ok(ua.encode(&zcash_network))
    }

    /// Generate UA directly from an FVK (used by DKG coordinator)
    pub fn generate_address_from_fvk(
        fvk: &FullViewingKey,
        network: zcash_primitives::consensus::Network,
    ) -> Result<String> {
        use zcash_address::Network;
        
        // Convert network types
        let zcash_network = match network {
            zcash_primitives::consensus::Network::MainNetwork => Network::Main,
            zcash_primitives::consensus::Network::TestNetwork => Network::Test,
        };
        
        // Get default address (diversifier index 0)
        let orchard_address = fvk.address_at(0u32, orchard::keys::Scope::External);
        
        // Build Unified Address
        let ua = Address::try_from_items(vec![
            Receiver::Orchard(orchard_address.to_raw_address_bytes()),
        ])?;
        
        Ok(ua.encode(&zcash_network))
    }

    /// Encode an FVK as a UFVK string (used by DKG coordinator)
    pub fn encode_ufvk(
        fvk: &FullViewingKey,
        network: zcash_primitives::consensus::Network,
    ) -> Result<String> {
        use zcash_address::{unified, Network};
        
        // Get FVK bytes
        let fvk_bytes = fvk.to_bytes();
        
        // Convert network types
        let zcash_network = match network {
            zcash_primitives::consensus::Network::MainNetwork => Network::Main,
            zcash_primitives::consensus::Network::TestNetwork => Network::Test,
        };
        
        // Create a unified container with the Orchard FVK
        let items = vec![unified::Fvk::Orchard(fvk_bytes)];
        let ufvk = unified::Ufvk::try_from_items(items)
            .map_err(|e| anyhow::anyhow!("Failed to create UFVK: {:?}", e))?;
        
        // Encode it
        Ok(ufvk.encode(&zcash_network))
    }
}