use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};
use tower_http::trace::TraceLayer;
use tracing::{info, debug, warn};
use base64::{Engine as _, engine::general_purpose};

/// HTTP-based network for distributed DKG
#[derive(Clone)]
pub struct HttpNetworkClient {
    node_id: u16,
    peers: HashMap<u16, String>, // node_id -> http://host:port
    client: reqwest::Client,
}

impl HttpNetworkClient {
    pub fn new(node_id: u16, peers: HashMap<u16, String>) -> Self {
        Self {
            node_id,
            peers,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Broadcast Round 1 package to ALL nodes (including self via localhost)
    pub async fn broadcast_round1(&self, data: Vec<u8>) -> Result<()> {
        let payload = DkgMessage {
            from_node: self.node_id,
            data: general_purpose::STANDARD.encode(&data),
        };

        // CRITICAL: Store on our own server FIRST (via localhost)
        let self_url = "http://localhost:8080/api/dkg/round1";
        debug!("Node {} storing its own Round 1 package locally", self.node_id);
        
        match self.client.post(self_url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!("✓ Node {} stored its own Round 1 package", self.node_id);
            }
            Ok(resp) => {
                anyhow::bail!("Node {} failed to store its own package: status {}", self.node_id, resp.status());
            }
            Err(e) => {
                anyhow::bail!("Node {} failed to store its own package: {}", self.node_id, e);
            }
        }

        // Then broadcast to peers
        for (peer_id, peer_addr) in &self.peers {
            let url = format!("{}/api/dkg/round1", peer_addr);
            debug!("Node {} sending Round 1 to node {} at {}", self.node_id, peer_id, url);
            
            let mut attempts = 0;
            loop {
                match self.client.post(&url).json(&payload).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        debug!("✓ Node {} successfully sent Round 1 to node {}", self.node_id, peer_id);
                        break;
                    }
                    Ok(resp) => {
                        warn!("Node {} received error from node {}: {}", self.node_id, peer_id, resp.status());
                    }
                    Err(e) => {
                        warn!("Node {} failed to send to node {}: {}", self.node_id, peer_id, e);
                    }
                }
                
                attempts += 1;
                if attempts >= 5 {
                    anyhow::bail!("Failed to send Round 1 to node {} after 5 attempts", peer_id);
                }
                
                sleep(Duration::from_millis(500 * attempts)).await;
            }
        }

        Ok(())
    }

    /// Receive Round 1 package from specific peer
    pub async fn receive_round1(&self, from_node: u16) -> Result<Vec<u8>> {
        // Query the peer's server for their package
        let peer_addr = self.peers.get(&from_node)
            .ok_or_else(|| anyhow::anyhow!("Peer {} not found", from_node))?;
        
        let url = format!("{}/api/dkg/round1/{}", peer_addr, from_node);
        
        for attempt in 0..100 {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let msg: DkgMessage = resp.json().await?;
                    let data = general_purpose::STANDARD.decode(&msg.data)?;
                    debug!("✓ Node {} received Round 1 from node {}", self.node_id, from_node);
                    return Ok(data);
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                    // Not ready yet, keep polling
                }
                Ok(resp) => {
                    warn!("Unexpected status from node {}: {}", from_node, resp.status());
                }
                Err(e) => {
                    debug!("Attempt {} to get Round 1 from node {}: {}", attempt + 1, from_node, e);
                }
            }
            
            if attempt < 99 {
                sleep(Duration::from_millis(200)).await;
            }
        }
        
        anyhow::bail!("Timeout waiting for Round 1 from node {}", from_node)
    }

    /// Send Round 2 package to specific peer (stores on OUR server for them to retrieve)
pub async fn send_round2(&self, to_node: u16, data: Vec<u8>) -> Result<()> {
    let payload = DkgRound2Message {
        from_node: self.node_id,
        to_node,
        data: general_purpose::STANDARD.encode(&data),
    };

    // CRITICAL: Always store on OUR OWN server (localhost)
    // The recipient will query our server to get their package
    let self_url = "http://localhost:8080/api/dkg/round2";
    
    debug!("Node {} storing Round 2 package for node {} locally", self.node_id, to_node);
    
    for attempt in 0..5 {
        match self.client.post(self_url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!("✓ Node {} successfully stored Round 2 for node {}", self.node_id, to_node);
                return Ok(());
            }
            Ok(resp) => {
                warn!("Attempt {} failed with status {}", attempt + 1, resp.status());
            }
            Err(e) => {
                warn!("Attempt {} failed: {}", attempt + 1, e);
            }
        }
        
        if attempt < 4 {
            sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
        }
    }
    
    anyhow::bail!("Failed to store Round 2 for node {} after 5 attempts", to_node)
}

    /// Receive Round 2 package from specific peer
    pub async fn receive_round2(&self, from_node: u16, to_node: u16) -> Result<Vec<u8>> {
        let peer_addr = self.peers.get(&from_node)
            .ok_or_else(|| anyhow::anyhow!("Peer {} not found", from_node))?;
        
        let url = format!("{}/api/dkg/round2/{}/{}", peer_addr, from_node, to_node);
        
        for attempt in 0..100 {
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let msg: DkgRound2Message = resp.json().await?;
                    let data = general_purpose::STANDARD.decode(&msg.data)?;
                    debug!("✓ Node {} received Round 2 from node {}", self.node_id, from_node);
                    return Ok(data);
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                    // Not ready yet
                }
                Ok(resp) => {
                    warn!("Unexpected status from node {}: {}", from_node, resp.status());
                }
                Err(e) => {
                    debug!("Attempt {} to get Round 2 from node {}: {}", attempt + 1, from_node, e);
                }
            }
            
            if attempt < 99 {
                sleep(Duration::from_millis(200)).await;
            }
        }
        
        anyhow::bail!("Timeout waiting for Round 2 from node {}", from_node)
    }
}

/// HTTP server for receiving DKG messages
#[derive(Clone)]
pub struct HttpNetworkServer {
    node_id: u16,
    storage: Arc<Mutex<ServerStorage>>,
}

#[derive(Default)]
struct ServerStorage {
    round1_messages: HashMap<u16, Vec<u8>>,
    round2_messages: HashMap<(u16, u16), Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DkgMessage {
    from_node: u16,
    data: String, // base64 encoded
}

#[derive(Debug, Serialize, Deserialize)]
struct DkgRound2Message {
    from_node: u16,
    to_node: u16,
    data: String, // base64 encoded
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    node_id: u16,
}

impl HttpNetworkServer {
    pub fn new(node_id: u16) -> Self {
        Self {
            node_id,
            storage: Arc::new(Mutex::new(ServerStorage::default())),
        }
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/health", get(health_check))
            .route("/api/dkg/round1", post(store_round1))
            .route("/api/dkg/round1/:from", get(get_round1))
            .route("/api/dkg/round2", post(store_round2))
            .route("/api/dkg/round2/:from/:to", get(get_round2))
            .layer(TraceLayer::new_for_http())
            .with_state(self.clone())
    }

    pub async fn serve(self, listen_addr: String) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
        info!("🌐 Node {} HTTP server listening on {}", self.node_id, listen_addr);
        
        axum::serve(listener, self.router()).await?;
        Ok(())
    }
}

async fn health_check(State(server): State<HttpNetworkServer>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        node_id: server.node_id,
    })
}

async fn store_round1(
    State(server): State<HttpNetworkServer>,
    Json(payload): Json<DkgMessage>,
) -> impl IntoResponse {
    let data = match general_purpose::STANDARD.decode(&payload.data) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("Invalid base64: {}", e)).into_response(),
    };

    let mut storage = server.storage.lock().unwrap();
    storage.round1_messages.insert(payload.from_node, data);
    
    debug!("✓ Node {} stored Round 1 from node {}", server.node_id, payload.from_node);
    StatusCode::OK.into_response()
}

async fn get_round1(
    State(server): State<HttpNetworkServer>,
    Path(from): Path<u16>,
) -> impl IntoResponse {
    let storage = server.storage.lock().unwrap();
    
    match storage.round1_messages.get(&from) {
        Some(data) => {
            let msg = DkgMessage {
                from_node: from,
                data: general_purpose::STANDARD.encode(data),
            };
            (StatusCode::OK, Json(msg)).into_response()
        }
        None => {
            (StatusCode::NOT_FOUND, "Package not found").into_response()
        }
    }
}

async fn store_round2(
    State(server): State<HttpNetworkServer>,
    Json(payload): Json<DkgRound2Message>,
) -> impl IntoResponse {
    let data = match general_purpose::STANDARD.decode(&payload.data) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("Invalid base64: {}", e)).into_response(),
    };

    let mut storage = server.storage.lock().unwrap();
    storage.round2_messages.insert((payload.from_node, payload.to_node), data);
    
    debug!("✓ Node {} stored Round 2 from node {} to node {}", 
           server.node_id, payload.from_node, payload.to_node);
    StatusCode::OK.into_response()
}

async fn get_round2(
    State(server): State<HttpNetworkServer>,
    Path((from, to)): Path<(u16, u16)>,
) -> impl IntoResponse {
    let storage = server.storage.lock().unwrap();
    
    match storage.round2_messages.get(&(from, to)) {
        Some(data) => {
            let msg = DkgRound2Message {
                from_node: from,
                to_node: to,
                data: general_purpose::STANDARD.encode(data),
            };
            (StatusCode::OK, Json(msg)).into_response()
        }
        None => {
            (StatusCode::NOT_FOUND, "Package not found").into_response()
        }
    }
}