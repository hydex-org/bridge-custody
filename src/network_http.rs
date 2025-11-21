use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use tracing::{debug, info};

#[derive(Clone)]
pub struct HttpNetworkClient {
    client: Client,
    node_id: u16,
    peers: HashMap<u16, String>,
}

impl HttpNetworkClient {
    pub fn new(node_id: u16, peers: HashMap<u16, String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            node_id,
            peers,
        }
    }

    /// Broadcast Round 1 DKG packages
    pub async fn broadcast_round1(&self, data: Vec<u8>) -> Result<()> {
        let payload = DkgMessage {
            from_node: self.node_id,
            data: general_purpose::STANDARD.encode(&data),
        };

        // CRITICAL: Store on our own server FIRST
        let self_url = "http://localhost:8080/api/dkg/round1";
        for attempt in 0..5 {
            match self.client.post(self_url).json(&payload).send().await {
                Ok(_) => break,
                Err(_) if attempt < 4 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                }
                Err(e) => anyhow::bail!("Failed to store Round 1 locally: {}", e),
            }
        }

        // Then broadcast to peers
        for (peer_id, peer_addr) in &self.peers {
            let url = format!("{}/api/dkg/round1", peer_addr);
            for attempt in 0..5 {
                match self.client.post(&url).json(&payload).send().await {
                    Ok(_) => {
                        debug!("Sent Round 1 to node {}", peer_id);
                        break;
                    }
                    Err(_) if attempt < 4 => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                    }
                    Err(e) => debug!("Failed to send to node {}: {}", peer_id, e),
                }
            }
        }
        Ok(())
    }

    /// Receive Round 1 packages from all peers
    pub async fn receive_round1(&self, from_node: u16) -> Result<Vec<u8>> {
        // Determine which server to query
        let url = if from_node == self.node_id {
            format!("http://localhost:8080/api/dkg/round1/{}", from_node)
        } else {
            let peer_addr = self
                .peers
                .get(&from_node)
                .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", from_node))?;
            format!("{}/api/dkg/round1/{}", peer_addr, from_node)
        };

        // Poll with retries
        for attempt in 0..30 {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let msg: DkgMessage = resp.json().await?;
                    let decoded = general_purpose::STANDARD.decode(&msg.data)?;
                    return Ok(decoded);
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                    if attempt < 29 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    }
                }
                _ if attempt < 29 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await
                }
                _ => anyhow::bail!("Timeout waiting for Round 1 from node {}", from_node),
            }
        }
        anyhow::bail!("Failed to receive Round 1 from node {}", from_node)
    }

    /// Send Round 2 package to a specific peer
    pub async fn send_round2(&self, to_node: u16, data: Vec<u8>) -> Result<()> {
        let payload = DkgRound2Message {
            from_node: self.node_id,
            to_node,
            data: general_purpose::STANDARD.encode(&data),
        };

        // CRITICAL: Always store on OUR OWN server (localhost)
        let self_url = "http://localhost:8080/api/dkg/round2";
        for attempt in 0..5 {
            match self.client.post(self_url).json(&payload).send().await {
                Ok(_) => return Ok(()),
                Err(_) if attempt < 4 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                }
                Err(e) if attempt == 4 => {
                    debug!("Failed to store Round 2: {}", e);
                }
                _ => {}
            }
        }

        anyhow::bail!(
            "Failed to store Round 2 for node {} after 5 attempts",
            to_node
        )
    }

    /// Receive Round 2 package from a peer
    pub async fn receive_round2(&self, from_node: u16) -> Result<Vec<u8>> {
        // Query the sender's server for the package they created for us
        let url = if from_node == self.node_id {
            format!(
                "http://localhost:8080/api/dkg/round2/{}/{}",
                from_node, self.node_id
            )
        } else {
            let peer_addr = self
                .peers
                .get(&from_node)
                .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", from_node))?;
            format!("{}/api/dkg/round2/{}/{}", peer_addr, from_node, self.node_id)
        };

        for attempt in 0..30 {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let msg: DkgRound2Message = resp.json().await?;
                    let decoded = general_purpose::STANDARD.decode(&msg.data)?;
                    return Ok(decoded);
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                    if attempt < 29 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    }
                }
                _ if attempt < 29 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await
                }
                _ => anyhow::bail!("Timeout waiting for Round 2 from node {}", from_node),
            }
        }
        anyhow::bail!("Failed to receive Round 2 from node {}", from_node)
    }

    /// Broadcast signing commitments (FROST Round 1)
    pub async fn broadcast_signing_commitments(&self, data: Vec<u8>) -> Result<()> {
        let payload = DkgMessage {
            from_node: self.node_id,
            data: general_purpose::STANDARD.encode(&data),
        };

        // Store on our own server first
        let self_url = "http://localhost:8080/api/signing/commitments";
        for attempt in 0..5 {
            match self.client.post(self_url).json(&payload).send().await {
                Ok(_) => break,
                Err(_) if attempt < 4 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                }
                Err(e) => anyhow::bail!("Failed to store commitment locally: {}", e),
            }
        }

        // Broadcast to peers
        for (peer_id, peer_addr) in &self.peers {
            let url = format!("{}/api/signing/commitments", peer_addr);
            for attempt in 0..5 {
                match self.client.post(&url).json(&payload).send().await {
                    Ok(_) => {
                        debug!("Sent commitment to node {}", peer_id);
                        break;
                    }
                    Err(_) if attempt < 4 => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                    }
                    Err(e) => debug!("Failed to send commitment to node {}: {}", peer_id, e),
                }
            }
        }
        Ok(())
    }

    /// Receive signing commitments from a peer
    pub async fn receive_signing_commitments(&self, from_node: u16) -> Result<Vec<u8>> {
        let url = if from_node == self.node_id {
            format!(
                "http://localhost:8080/api/signing/commitments/{}",
                from_node
            )
        } else {
            let peer_addr = self
                .peers
                .get(&from_node)
                .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", from_node))?;
            format!("{}/api/signing/commitments/{}", peer_addr, from_node)
        };

        for attempt in 0..30 {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let msg: DkgMessage = resp.json().await?;
                    let decoded = general_purpose::STANDARD.decode(&msg.data)?;
                    return Ok(decoded);
                }
                _ if attempt < 29 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await
                }
                _ => anyhow::bail!("Timeout waiting for commitment from node {}", from_node),
            }
        }
        unreachable!()
    }

    /// Broadcast signature shares (FROST Round 2)
    pub async fn broadcast_signature_shares(&self, data: Vec<u8>) -> Result<()> {
        let payload = DkgMessage {
            from_node: self.node_id,
            data: general_purpose::STANDARD.encode(&data),
        };

        // Store on our own server first
        let self_url = "http://localhost:8080/api/signing/shares";
        for attempt in 0..5 {
            match self.client.post(self_url).json(&payload).send().await {
                Ok(_) => break,
                Err(_) if attempt < 4 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                }
                Err(e) => anyhow::bail!("Failed to store share locally: {}", e),
            }
        }

        // Broadcast to peers
        for (peer_id, peer_addr) in &self.peers {
            let url = format!("{}/api/signing/shares", peer_addr);
            for attempt in 0..5 {
                match self.client.post(&url).json(&payload).send().await {
                    Ok(_) => {
                        debug!("Sent signature share to node {}", peer_id);
                        break;
                    }
                    Err(_) if attempt < 4 => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await
                    }
                    Err(e) => debug!("Failed to send share to node {}: {}", peer_id, e),
                }
            }
        }
        Ok(())
    }

    /// Receive signature shares from a peer
    pub async fn receive_signature_shares(&self, from_node: u16) -> Result<Vec<u8>> {
        let url = if from_node == self.node_id {
            format!("http://localhost:8080/api/signing/shares/{}", from_node)
        } else {
            let peer_addr = self
                .peers
                .get(&from_node)
                .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", from_node))?;
            format!("{}/api/signing/shares/{}", peer_addr, from_node)
        };

        for attempt in 0..30 {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let msg: DkgMessage = resp.json().await?;
                    let decoded = general_purpose::STANDARD.decode(&msg.data)?;
                    return Ok(decoded);
                }
                _ if attempt < 29 => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await
                }
                _ => anyhow::bail!("Timeout waiting for share from node {}", from_node),
            }
        }
        unreachable!()
    }
}

// HTTP Server for receiving DKG/Signing messages
#[derive(Clone)]
pub struct HttpNetworkServer {
    node_id: u16,
    round1_packages: Arc<Mutex<HashMap<u16, String>>>,
    round2_packages: Arc<Mutex<HashMap<(u16, u16), String>>>,
    commitments: Arc<Mutex<HashMap<u16, String>>>,
    signature_shares: Arc<Mutex<HashMap<u16, String>>>,
}

#[derive(Clone)]
struct ServerState {
    node_id: u16,
    round1_packages: Arc<Mutex<HashMap<u16, String>>>,
    round2_packages: Arc<Mutex<HashMap<(u16, u16), String>>>,
    commitments: Arc<Mutex<HashMap<u16, String>>>,
    signature_shares: Arc<Mutex<HashMap<u16, String>>>,
}

impl HttpNetworkServer {
    pub fn new(node_id: u16) -> Self {
        Self {
            node_id,
            round1_packages: Arc::new(Mutex::new(HashMap::new())),
            round2_packages: Arc::new(Mutex::new(HashMap::new())),
            commitments: Arc::new(Mutex::new(HashMap::new())),
            signature_shares: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn serve(self, addr: String) -> Result<()> {
        let state = ServerState {
            node_id: self.node_id,
            round1_packages: self.round1_packages.clone(),
            round2_packages: self.round2_packages.clone(),
            commitments: self.commitments.clone(),
            signature_shares: self.signature_shares.clone(),
        };

        let app = Router::new()
            .route("/health", get(health_check))
            .route("/api/dkg/round1", post(handle_round1))
            .route("/api/dkg/round1/:node_id", get(get_round1))
            .route("/api/dkg/round2", post(handle_round2))
            .route("/api/dkg/round2/:from/:to", get(get_round2))
            .route("/api/signing/commitments", post(handle_signing_commitments))
            .route(
                "/api/signing/commitments/:node_id",
                get(get_signing_commitments),
            )
            .route("/api/signing/shares", post(handle_signing_shares))
            .route("/api/signing/shares/:node_id", get(get_signing_shares))
            .layer(TraceLayer::new_for_http())
            .with_state(Arc::new(state));

        info!("🌐 HTTP server listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

// Message types
#[derive(Debug, Serialize, Deserialize)]
struct DkgMessage {
    from_node: u16,
    data: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DkgRound2Message {
    from_node: u16,
    to_node: u16,
    data: String,
}

// Handler functions
async fn health_check() -> &'static str {
    "OK"
}

async fn handle_round1(
    State(state): State<Arc<ServerState>>,
    Json(msg): Json<DkgMessage>,
) -> axum::http::StatusCode {
    let mut storage = state.round1_packages.lock().await;
    storage.insert(msg.from_node, msg.data);
    axum::http::StatusCode::OK
}

async fn get_round1(
    State(state): State<Arc<ServerState>>,
    Path(node_id): Path<u16>,
) -> Result<Json<DkgMessage>, axum::http::StatusCode> {
    let storage = state.round1_packages.lock().await;
    if let Some(data) = storage.get(&node_id) {
        Ok(Json(DkgMessage {
            from_node: node_id,
            data: data.clone(),
        }))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn handle_round2(
    State(state): State<Arc<ServerState>>,
    Json(msg): Json<DkgRound2Message>,
) -> axum::http::StatusCode {
    let mut storage = state.round2_packages.lock().await;
    storage.insert((msg.from_node, msg.to_node), msg.data);
    axum::http::StatusCode::OK
}

async fn get_round2(
    State(state): State<Arc<ServerState>>,
    Path((from, to)): Path<(u16, u16)>,
) -> Result<Json<DkgRound2Message>, axum::http::StatusCode> {
    let storage = state.round2_packages.lock().await;
    if let Some(data) = storage.get(&(from, to)) {
        Ok(Json(DkgRound2Message {
            from_node: from,
            to_node: to,
            data: data.clone(),
        }))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

// FROST Signing Commitment handlers
async fn handle_signing_commitments(
    State(state): State<Arc<ServerState>>,
    Json(msg): Json<DkgMessage>,
) -> axum::http::StatusCode {
    let mut storage = state.commitments.lock().await;
    storage.insert(msg.from_node, msg.data);
    axum::http::StatusCode::OK
}

async fn get_signing_commitments(
    State(state): State<Arc<ServerState>>,
    Path(node_id): Path<u16>,
) -> Result<Json<DkgMessage>, axum::http::StatusCode> {
    let storage = state.commitments.lock().await;
    if let Some(data) = storage.get(&node_id) {
        Ok(Json(DkgMessage {
            from_node: node_id,
            data: data.clone(),
        }))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

// FROST Signature Share handlers
async fn handle_signing_shares(
    State(state): State<Arc<ServerState>>,
    Json(msg): Json<DkgMessage>,
) -> axum::http::StatusCode {
    let mut storage = state.signature_shares.lock().await;
    storage.insert(msg.from_node, msg.data);
    axum::http::StatusCode::OK
}

async fn get_signing_shares(
    State(state): State<Arc<ServerState>>,
    Path(node_id): Path<u16>,
) -> Result<Json<DkgMessage>, axum::http::StatusCode> {
    let storage = state.signature_shares.lock().await;
    if let Some(data) = storage.get(&node_id) {
        Ok(Json(DkgMessage {
            from_node: node_id,
            data: data.clone(),
        }))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}