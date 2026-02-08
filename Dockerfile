# ==============================================================================
# Stage 1: Builder - Build application
# ==============================================================================
FROM rust:1-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# Enable sqlx offline mode (uses pre-generated .sqlx cache)
ENV SQLX_OFFLINE=true

# --- Phase 1: Cache dependency compilation ---
# Copy only manifests and lock file
COPY Cargo.toml Cargo.lock ./
COPY crates/api-server/Cargo.toml crates/api-server/Cargo.toml
COPY crates/arb-monitor/Cargo.toml crates/arb-monitor/Cargo.toml
COPY crates/auth/Cargo.toml crates/auth/Cargo.toml
COPY crates/backtester/Cargo.toml crates/backtester/Cargo.toml
COPY crates/bot-scanner/Cargo.toml crates/bot-scanner/Cargo.toml
COPY crates/polymarket-core/Cargo.toml crates/polymarket-core/Cargo.toml
COPY crates/risk-manager/Cargo.toml crates/risk-manager/Cargo.toml
COPY crates/trading-engine/Cargo.toml crates/trading-engine/Cargo.toml
COPY crates/wallet-tracker/Cargo.toml crates/wallet-tracker/Cargo.toml

# Create dummy source files so cargo can resolve the workspace
RUN mkdir -p src && echo "" > src/lib.rs \
    && mkdir -p crates/api-server/src && echo "fn main() {}" > crates/api-server/src/main.rs && echo "" > crates/api-server/src/lib.rs \
    && mkdir -p crates/arb-monitor/src && echo "fn main() {}" > crates/arb-monitor/src/main.rs \
    && mkdir -p crates/auth/src && echo "" > crates/auth/src/lib.rs \
    && mkdir -p crates/backtester/src && echo "" > crates/backtester/src/lib.rs \
    && mkdir -p crates/bot-scanner/src && echo "fn main() {}" > crates/bot-scanner/src/main.rs \
    && mkdir -p crates/polymarket-core/src && echo "" > crates/polymarket-core/src/lib.rs \
    && mkdir -p crates/risk-manager/src && echo "" > crates/risk-manager/src/lib.rs \
    && mkdir -p crates/trading-engine/src && echo "" > crates/trading-engine/src/lib.rs \
    && mkdir -p crates/wallet-tracker/src && echo "" > crates/wallet-tracker/src/lib.rs

# Build dependencies only (this layer is cached unless Cargo.toml/Cargo.lock change)
RUN cargo build --release --workspace 2>/dev/null || true

# --- Phase 2: Build real source ---
# Copy actual source code (invalidates cache only when source changes)
COPY . .

# Touch source files to ensure cargo rebuilds workspace crates
RUN touch src/lib.rs \
    && touch crates/api-server/src/main.rs crates/api-server/src/lib.rs \
    && touch crates/arb-monitor/src/main.rs \
    && touch crates/auth/src/lib.rs \
    && touch crates/backtester/src/lib.rs \
    && touch crates/bot-scanner/src/main.rs \
    && touch crates/polymarket-core/src/lib.rs \
    && touch crates/risk-manager/src/lib.rs \
    && touch crates/trading-engine/src/lib.rs \
    && touch crates/wallet-tracker/src/lib.rs

# Build all binaries in release mode (only workspace crates recompile)
RUN cargo build --release --workspace

# ==============================================================================
# Stage 2: Runtime - Minimal production image
# Cache bust: 2026-01-15-v2
# ==============================================================================
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies (cache bust: v2)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    curl \
    bash \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash appuser

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /app/target/release/api-server /app/api-server
COPY --from=builder /app/target/release/arb-monitor /app/arb-monitor
COPY --from=builder /app/target/release/bot-scanner /app/bot-scanner

# Copy migrations
COPY --from=builder /app/migrations /app/migrations

# Create entrypoint script that reads SERVICE env var
RUN echo '#!/bin/bash\n\
SERVICE=${SERVICE:-api-server}\n\
echo "Starting service: $SERVICE"\n\
exec ./$SERVICE' > /app/entrypoint.sh && chmod +x /app/entrypoint.sh

# Set ownership
RUN chown -R appuser:appuser /app

USER appuser

# Default environment variables
ENV RUST_LOG=info
ENV API_HOST=0.0.0.0
ENV API_PORT=3000
ENV SERVICE=api-server

EXPOSE 3000

# Use entrypoint script
ENTRYPOINT ["/app/entrypoint.sh"]
