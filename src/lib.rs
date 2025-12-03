//! Bridge Custody - MPC Node for Zcash-Solana Bridge
//!
//! ## Architecture (per Hydex spec)
//!
//! MPC nodes are responsible for:
//! - DKG (Distributed Key Generation) for FROST threshold signing
//! - Signing Zcash withdrawals via 2-of-3 FROST
//! - Provisioning the enclave with UFVK after DKG
//!
//! MPC nodes do NOT handle:
//! - Address generation (that's the enclave's job)
//! - Viewing keys after initial provisioning
//! - Deposit detection (that's the enclave's job)
//!
//! The UFVK is only passed to the enclave during provisioning.
//! After that, only the enclave holds and uses the viewing key.

pub mod types;
// pub mod mpc_node;  // Disabled - uses old in-memory NetworkClient
pub mod dkg_coordinator;
pub mod frost_signer;
pub mod network;
pub mod network_http;
pub mod zcash_client;
pub mod ua_builder;
pub mod tx_builder;
pub mod api;
pub mod orchard_frost;

// LEGACY: Address generation should be done by the enclave, not MPC nodes.
// This module is kept for testing/debugging only.
#[deprecated(note = "Address generation should be done by the enclave via /v1/deposit-intents")]
pub mod address_manager;

// Attestation flow - MPC nodes poll enclave for attestations and submit to Solana
pub mod enclave_client;
pub mod solana_client;
pub mod attestation_service;