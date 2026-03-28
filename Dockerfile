# ── Builder stage ─────────────────────────────────────────────────────────────
FROM rust:1.87-slim AS builder

WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --locked
RUN rm -f target/release/deps/rausu*

# Build the real source
COPY src ./src
RUN cargo build --release --locked

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/rausu /usr/local/bin/rausu

EXPOSE 4000

ENTRYPOINT ["rausu"]
CMD ["--config", "config.yaml"]
