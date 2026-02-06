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

# Copy source code
COPY . .

# Enable sqlx offline mode (uses pre-generated .sqlx cache)
ENV SQLX_OFFLINE=true

# Build all binaries in release mode
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
