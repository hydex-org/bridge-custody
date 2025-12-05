FROM rust:latest AS builder

WORKDIR /app

# Copy everything from bridge-custody (including vendored frost-pallas)
COPY bridge-custody/ .
COPY frost-pallas/ /frost-pallas/

# Build in release mode
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y libssl3 ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/mpc-node /usr/local/bin/mpc-node

# Create data directory
RUN mkdir -p /data

# Health check
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Default command
CMD ["mpc-node", "--config", "/app/config/node.toml"]
