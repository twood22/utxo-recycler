# Build stage
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /app

# Install dependencies for building
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy main to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source code
COPY src ./src
COPY templates ./templates
COPY migrations ./migrations

# Build the real application
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies including Tor and netcat for health checks
RUN apt-get update && apt-get install -y ca-certificates tor netcat-openbsd && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/utxo-recycler /app/utxo-recycler

# Copy templates, migrations, and static files
COPY --from=builder /app/templates ./templates
COPY --from=builder /app/migrations ./migrations
COPY static ./static

# Copy startup script
COPY start.sh /app/start.sh
RUN chmod +x /app/start.sh

ENV SERVER_HOST=0.0.0.0
ENV SERVER_PORT=8080

EXPOSE 8080

CMD ["/app/start.sh"]
