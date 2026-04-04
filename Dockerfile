# ── Builder stage ─────────────────────────────────────────────────────────────
FROM rust:1.94-slim AS builder

# ring crate (used by rustls/jsonwebtoken) needs a C compiler and perl
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential perl pkg-config \
    && rm -rf /var/lib/apt/lists/*

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
FROM gcr.io/distroless/cc-debian12:latest AS runtime

WORKDIR /app

COPY --from=builder /app/target/release/rausu /usr/local/bin/rausu

USER nonroot

EXPOSE 4000

ENTRYPOINT ["rausu"]
CMD ["--config", "config.yaml"]
