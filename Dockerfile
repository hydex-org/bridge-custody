FROM rust:1.75 as builder

WORKDIR /app

# Copy frost-pallas first (dependency)
COPY frost-pallas /app/frost-pallas

# Copy bridge-custody
COPY bridge-custody /app/bridge-custody

WORKDIR /app/bridge-custody

# Build the project
RUN cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/bridge-custody/target/release/mpc-node /usr/local/bin/mpc-node
COPY --from=builder /app/bridge-custody/config /config

ENTRYPOINT ["/usr/local/bin/mpc-node"]