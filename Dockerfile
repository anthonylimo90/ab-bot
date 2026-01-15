# ==============================================================================
# Stage 1: Builder - Build application
# ==============================================================================
FROM rust:1.85-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy source code
COPY . .

# Pin home crate to version compatible with Rust 1.85
RUN cargo update home --precise 0.5.9

# Enable sqlx offline mode (uses pre-generated .sqlx cache)
ENV SQLX_OFFLINE=true

# Build all binaries in release mode
RUN cargo build --release --workspace

# ==============================================================================
# Stage 2: Runtime - Minimal production image
# ==============================================================================
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    curl \
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

# Set ownership
RUN chown -R appuser:appuser /app

USER appuser

# Default environment variables
ENV RUST_LOG=info
ENV API_HOST=0.0.0.0
ENV API_PORT=3000

EXPOSE 3000

# Health check disabled - Railway handles healthchecks
# HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
#     CMD curl -f http://localhost:3000/health || exit 1

# Default command
CMD ["./api-server"]

# ==============================================================================
# Alternative targets for specific services
# ==============================================================================

# API Server target
FROM runtime AS api-server
CMD ["./api-server"]

# Arb Monitor target
FROM runtime AS arb-monitor
CMD ["./arb-monitor"]

# Bot Scanner target
FROM runtime AS bot-scanner
CMD ["./bot-scanner"]
