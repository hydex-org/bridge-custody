# 4.2 Arcium Enclave (TEE)

## 4.2.1 Purpose

The Arcium Enclave provides a confidential computing environment for privacy-preserving bridge operations between Zcash and Solana. The enclave implementation (`ufvk-scanner`) is a stateless Rust application designed to run inside Arcium's TEE infrastructure.

### Private deposit detection

Scans Zcash blocks inside a confidential computing environment and identifies notes sent to the bridge.

**Implementation Details:**
- Uses gRPC client to connect to Zcash lightwalletd nodes (default: `https://testnet.zec.rocks:443`)
- Streams `CompactBlock` messages containing `CompactOrchardAction` data structures
- Processes blocks in batches of 100 to avoid timeout issues
- Each `CompactOrchardAction` contains:
  - 32-byte nullifier (unique identifier preventing double-spends)
  - 32-byte note commitment (cmx)
  - 32-byte ephemeral public key
  - 52-byte encrypted note ciphertext
- Performs trial decryption using prepared Incoming Viewing Key (IVK) for optimized performance
- Successfully decrypted notes reveal the note value (u64) in zatoshis

### Attestation generation

Produces signed, verifiable attestations proving deposits occurred.

**Implementation Context:**
- The `ufvk-scanner` application itself does not contain attestation generation code
- Attestation is handled by the Arcium platform layer surrounding the enclave
- The scanner reports detected deposits which are then incorporated into attestations by the TEE infrastructure
- Integration includes an `emit_orchard` API endpoint (`http://localhost:8080/zec/emit_orchard`) that receives:
  ```json
  {
    "data": "<block_hash_bytes>",
    "height": <block_height>
  }
  ```
- This reporting mechanism allows external orchestration components to track scanning progress and coordinate attestation generation

### Minimal state retention

Holds only the key material and short-lived data required to match deposits to intents.
No historical Solana↔Zcash mappings are stored after minting.

**State Management:**
- **In-Memory Nullifier Tracking:** Uses `HashSet<Vec<u8>>` to track seen nullifiers within a scanning session
  - Prevents processing duplicate notes
  - Detects potential double-spend attempts
  - Cleared when scanner restarts (stateless design)
- **No Persistent Storage:** Application does not write to disk or maintain databases
- **Ephemeral Key Material:** FVK is provided via command-line argument and held only in process memory
- **No Historical Records:** After processing a block, only the nullifier set is retained; no transaction history, user mappings, or metadata persists

### Confidential computation integration

Uses Arcium's encrypted-instruction system to privately validate attestations and manage burn intents on-chain.

**Architecture:**
- The `ufvk-scanner` runs as a **workload** inside an Arcium-managed TEE
- TEE infrastructure provides:
  - Remote attestation capabilities
  - Secure key provisioning (though current implementation uses CLI args)
  - Encrypted I/O channels
  - Quote generation for proving enclave identity
- No SGX/SEV-specific code in the scanner itself; portability maintained through Arcium's abstraction layer
- Async runtime (Tokio) enables concurrent block processing without blocking

## 4.2.2 What It Stores

### Full Viewing Key (FVK)

Required for trial decryption of shielded notes.

**Technical Specifications:**
- **Format:** 96-byte Orchard Full Viewing Key extracted from Unified Full Viewing Key (UFVK)
- **Derivation Chain:**
  1. UFVK is decoded using `zcash_address::unified::Ufvk::decode()`
  2. Orchard component (96 bytes) is extracted from the unified structure
  3. FVK is parsed using `orchard::keys::FullViewingKey::from_bytes()`
  4. Incoming Viewing Key (IVK) is derived for external scope: `fvk.to_ivk(Scope::External)`
  5. IVK is prepared for efficient decryption: `PreparedIncomingViewingKey::new(&ivk)`
- **Usage:** The PreparedIVK contains pre-computed cryptographic tables that accelerate trial decryption operations
- **Scope:** External scope IVK specifically targets notes sent from external parties (not change notes)
- **Lifecycle:** Loaded at startup, held in memory for the session duration, discarded on shutdown

**Cryptographic Libraries:**
- `orchard` crate: Core Orchard protocol implementation
- `zcash_primitives`: Key derivation and address handling
- `zcash_note_encryption`: Trial decryption primitives
- `pasta_curves`: Pallas/Vesta elliptic curve operations

### Enclave signing key

Used to sign deposit attestations.

**Current Implementation:**
- Not explicitly managed within the `ufvk-scanner` codebase
- Provisioned and managed by the Arcium platform layer
- Used by the TEE infrastructure to sign attestation payloads after the scanner identifies deposits
- Public key component registered with the Solana program for verification

**Expected Properties:**
- Algorithm: Likely Ed25519 or ECDSA (Arcium platform-dependent)
- Key Generation: Performed during enclave initialization or provisioned via secure channels
- Rotation: Supported through enclave re-deployment without requiring historical state

### Ephemeral intent-matching state

Only stored temporarily to create mint attestations.

**Implementation Details:**
- **Nullifier Set:** `HashSet<Vec<u8>>` tracking nullifiers of successfully decrypted notes
  - Prevents double-processing within a session
  - Size grows proportionally to detected deposits
  - Cleared on restart (not persisted)
- **Block Range State:** Current scanning position (start/end heights)
  - Managed by the main scanning loop
  - Can be restarted from any block height via CLI arguments
- **Progress Indicators:** Visual CLI feedback using `indicatif` crate
  - Not part of the security model
  - Provides operator visibility into scanning progress

**What Is NOT Stored:**
- No deposit-to-intent mappings
- No user account associations
- No historical transaction logs
- No Solana address to Zcash address mappings
- No balance information

(It does not store deposit history, mapping tables, or any long-term user metadata.)

## 4.2.3 How It Works

Connects to lightwalletd
Retrieves compact Orchard/Sapling blocks.

Trial-decrypts with the FVK
Detects notes addressed to the bridge's UA.

Matches notes to active deposit intents
Uses only short-lived in-memory data.

Creates attestation
Payload includes note commitment, nullifier, amount, block height, and target Solana address.

Signs attestation inside the enclave
Ensures authenticity and prevents operator forgery.

Returns attestation to the orchestrator
The orchestrator forwards it to the Solana program's mint_with_attestation.

## 4.2.4 Arcium Encrypted Instructions

Arcium provides confidential compute instructions defined in encrypted-ixs/src/lib.rs.
These execute off-chain inside the TEE, and only encrypted results are consumed on-chain.

**Note:** The encrypted instructions module is part of the broader Arcium bridge architecture, not included in the `ufvk-scanner` repository. These instructions are executed by separate Arcium runtime components that consume the scanner's output.

### • verify_attestation

Validates enclave-produced deposit attestations.

**Input (AttestationInput):**
- `note_commitment` — 32-byte Orchard note commitment (cmx) from the CompactOrchardAction
- `amount` — Note value in zatoshis (u64), extracted via trial decryption
- `recipient_solana` — Target Solana public key for minting wZEC (32 bytes)
- `block_height` — Zcash block height where the note was included (u64)
- `enclave_signature` — Digital signature produced by the enclave signing key
- `enclave_pubkey` — Public key corresponding to the enclave signing key

**Output:**
- `bool` (valid or invalid attestation)

**Purpose:**
This ensures Solana never handles plaintext attestation data — only encrypted verification results. The Solana program can verify attestations without exposing deposit details to the public blockchain.

**Cryptographic Flow:**
1. Reconstruct attestation payload from input fields
2. Verify signature using `enclave_pubkey`
3. Check that `enclave_pubkey` matches the registered enclave authority in the Solana program
4. Validate that `note_commitment` has not been previously used (prevent replay attacks)
5. Return boolean result to the Solana program

### • create_burn_intent

Builds an encrypted withdrawal record when a user burns wZEC.

**Input (BurnIntentInput):**
- `user` — Solana public key of the user burning wZEC (32 bytes)
- `amount` — Amount of wZEC being burned, in lamports (u64)
- `zcash_address` — Destination Zcash Unified Address (up to 256 bytes)
- `address_len` — Actual length of the Zcash address (usize)

**Output (BurnIntentOutput):**
- `burn_id` — Unique identifier for this withdrawal request (u64 or u128)
- `user` — Echo of the user's Solana pubkey
- `amount` — Echo of the burn amount
- `zcash_address` — Encrypted or plaintext Zcash address (implementation-dependent)
- `address_len` — Echo of the address length
- `status` — Withdrawal state: 0 = Pending, 1 = Processing, 2 = Completed

**Purpose:**
This protects the user's Zcash withdrawal address and amount from being exposed on-chain. The Solana program only stores encrypted burn intent records, while the enclave (and potentially MPC nodes) can decrypt and process the actual withdrawal.

**Integration with ufvk-scanner:**
- The scanner does not process burn intents directly
- Burn intents are created when users call the Solana program's burn instruction
- The enclave infrastructure stores these intents in encrypted form
- A separate component (possibly MPC coordinators) retrieves and processes burn intents to construct Zcash transactions

### • update_burn_intent

Finalizes the encrypted withdrawal record after MPC produces the Zcash transaction.

**Input (UpdateBurnIntentInput):**
- `existing burn_intent` — Reference to the burn intent record being updated
- `zcash_txid` — Transaction ID of the Zcash transaction sending funds to the user (32 bytes)
- `new_status` — Updated status code (typically 2 = Completed)

**Output:**
- Updated `BurnIntentOutput` with finalized fields:
  - `status` set to Completed
  - `zcash_txid` embedded (encrypted or as metadata)

**Purpose:**
This allows the Solana program to track withdrawal progression without ever revealing Zcash addresses or TX metadata publicly. Users can query their burn intent status to confirm completion, but the blockchain only stores encrypted or hashed references to the actual Zcash transaction.

**Lifecycle:**
1. User calls `burn` on Solana program → `create_burn_intent` executed → Pending state
2. MPC nodes retrieve burn intent from enclave → Construct Zcash transaction → Broadcast to Zcash network
3. MPC calls `update_burn_intent` with txid → Status set to Processing
4. After confirmation, final `update_burn_intent` → Status set to Completed

## 4.2.5 Remote Attestation

Remote attestation establishes a cryptographic chain of trust from the enclave hardware to the Solana program, ensuring that only legitimate, unmodified enclave code can produce valid deposit attestations.

### Tied to enclave measurement (MRENCLAVE)

Confirms the code and configuration match the expected version.

**Technical Details:**
- **MRENCLAVE:** SHA-256 hash of the enclave binary, configuration, and initial state
  - Generated during enclave build process
  - Changes with any code modification, dependency update, or configuration change
  - Serves as a unique fingerprint for the exact enclave version
- **Quote Generation:**
  - When the enclave starts, the TEE hardware produces an attestation quote
  - Quote includes MRENCLAVE, MRSIGNER (signer identity), security version numbers, and platform details
  - Signed by the hardware platform's attestation key (e.g., Intel EPID or DCAP for SGX, AMD SEV-SNP attestation)
- **Verification Service:**
  - Quote is sent to Intel Attestation Service (IAS) or DCAP verifier
  - Returns a verification report certifying the quote's authenticity
  - Report is cryptographically signed by Intel/AMD and can be verified on-chain or off-chain

**Security Guarantees:**
- Prevents running tampered or backdoored enclave binaries
- Ensures reproducible builds: Anyone can recompile the source and verify the MRENCLAVE matches
- Detects configuration changes (e.g., debug mode enabled)

### Verified by the Solana program

Prevents malicious or spoofed enclaves from producing mintable attestations.

**On-Chain Verification Flow:**
1. **Enclave Registration:**
   - Operator submits the enclave's remote attestation quote and public key to the Solana program
   - Solana program (or off-chain verifier feeding the program) validates:
     - Quote signature authenticity
     - MRENCLAVE matches the expected value
     - Security version is not revoked
     - No debug flags are set
   - Public key is stored as the `authorized_enclave_pubkey`

2. **Attestation Verification:**
   - User submits deposit attestation to `mint_with_attestation` instruction
   - Solana program checks:
     - Signature on attestation verifies against `authorized_enclave_pubkey`
     - Note commitment has not been previously minted
     - Block height is reasonable (not too far in past/future)
   - Only attestations signed by the registered enclave can trigger minting

3. **Continuous Validation:**
   - Enclave rotation requires submitting a new attestation quote
   - Old enclave keys can be revoked on-chain
   - Program can enforce minimum security version numbers

**Cryptographic Binding:**
- The enclave signing key never leaves the TEE
- Private key is generated inside the enclave or sealed to the enclave's identity
- Even the operator cannot extract or misuse the signing key
- This guarantees that attestations genuinely originated from the verified enclave code

### Establishes trust boundary

Solana only accepts attestations from the registered enclave authority key.

**Trust Model:**
- **Trusted:** TEE hardware manufacturer (Intel, AMD), enclave code audited and published
- **Untrusted:** Network operators, cloud providers, operating system
- **Threat Mitigation:**
  - Operator cannot forge deposit attestations without the enclave signing key
  - Compromised OS cannot read enclave memory or tamper with execution
  - Side-channel attacks mitigated by TEE isolation and constant-time cryptographic operations

**Decentralization Considerations:**
- Multiple independent enclaves can be authorized on-chain
- Each enclave maintains its own signing key
- Threshold schemes or multi-sig attestations could be implemented for redundancy
- If one enclave is compromised or revoked, others continue operating

**Auditability:**
- Enclave source code can be published for public review
- Reproducible builds allow third parties to verify MRENCLAVE
- All attestations are recorded on Solana's immutable ledger
- Suspicious patterns (e.g., duplicate note commitments, invalid nullifiers) can trigger alerts

## 4.2.6 Failure Modes

The enclave is designed to fail safely and recover gracefully from various network and operational issues.

### Missing or delayed Zcash blocks

Network delays slow detection but cannot cause incorrect results.

**Behavior:**
- **gRPC Connection Timeout:**
  - The `tonic` gRPC client has configurable timeout settings
  - If lightwalletd is unreachable, connection attempts fail with clear errors
  - Scanner logs the error and can retry with exponential backoff
- **Incomplete Block Stream:**
  - When requesting a block range (e.g., 1000-1100), lightwalletd may timeout mid-stream
  - Scanner processes blocks received so far, then retries from the last successfully processed height
  - Batch size of 100 blocks reduces likelihood of incomplete streams
- **Stale Data:**
  - If lightwalletd lags behind the Zcash network, the scanner sees older blocks
  - This delays deposit detection but does not create false positives
  - Eventually consistent: Once lightwalletd syncs, the enclave processes the missing blocks

**Error Handling:**
- Uses `anyhow::Result` for propagating errors with context
- Critical errors (e.g., invalid FVK) cause immediate shutdown
- Network errors trigger retries without terminating the scanner
- Detailed logging via `tracing` crate for debugging

### Late detection mode

If the enclave catches up after a delay, it still produces valid attestations.

**Recovery Scenarios:**
1. **Enclave Downtime:**
   - Scanner is offline for hours or days
   - On restart, operator specifies last scanned block height via CLI
   - Scanner resumes from that point and processes all missed blocks
   - Attestations are still valid as long as the note commitments haven't been used

2. **Out-of-Order Processing:**
   - Lightwalletd may reorg or provide blocks out of order (rare)
   - Scanner processes each block independently
   - Nullifier deduplication prevents double-processing
   - Final attestations correctly reference the canonical block height

3. **Catch-Up Performance:**
   - Trial decryption is the bottleneck (cryptographic operations)
   - PreparedIncomingViewingKey optimization speeds up decryption by ~30%
   - On modern hardware, scanner can process ~1000 blocks/minute
   - Full testnet sync (from genesis) takes several hours; mainnet longer

**Temporal Guarantees:**
- Attestations include `block_height` field
- Solana program can enforce maximum age (e.g., reject attestations for blocks older than 1000 blocks)
- This prevents replay attacks using old, valid attestations after the note has already been minted

### Enclave rotation

New enclave instances can reuse the FVK and enclave pubkey without needing historical state.

**Stateless Design Benefits:**
- **No Migration Overhead:** New enclave starts fresh with just the FVK
- **Horizontal Scaling:** Multiple enclaves can scan different block ranges in parallel
- **Disaster Recovery:** If an enclave crashes, simply restart with the same FVK
- **Zero Downtime Rotation:**
  1. Operator deploys new enclave with updated code
  2. Generates remote attestation quote for new enclave
  3. Registers new enclave pubkey on Solana (via program upgrade authority)
  4. Old enclave key is revoked
  5. Deposits continue seamlessly

**Key Rotation Procedure:**
1. **New Enclave Deployment:**
   - Build new enclave binary with updated MRENCLAVE
   - Deploy to Arcium infrastructure
   - Enclave generates a new signing key pair inside the TEE
   - Produce remote attestation quote binding pubkey to MRENCLAVE

2. **On-Chain Registration:**
   - Submit quote to Solana program's `register_enclave` instruction
   - Program verifies quote and stores new `authorized_enclave_pubkey`
   - Optionally maintain multiple active keys for redundancy

3. **FVK Continuity:**
   - FVK remains the same across enclave rotations
   - New enclave receives the same FVK via secure provisioning (currently CLI, ideally sealed storage)
   - Scanning continues from the current block height; no need to rescan history

4. **Revocation:**
   - Old enclave keys marked as revoked in Solana program
   - Attestations signed by revoked keys are rejected
   - Grace period can allow dual operation during transition

**Security Considerations:**
- Key rotation does not expose the FVK outside the TEE boundary
- MRENCLAVE change is detectable by on-chain verifiers
- Rotation events are recorded on-chain for audit trails
- Compromised old keys cannot retroactively invalidate new attestations
