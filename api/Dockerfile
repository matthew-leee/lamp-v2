# Multi-stage production Dockerfile for the Rust API (OPS-01, BUDGET-01).
FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --bin lamp-api

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home --shell /usr/sbin/nologin appuser

WORKDIR /app
COPY --from=builder /build/target/release/lamp-api /app/lamp-api
COPY migrations /app/migrations

USER appuser
EXPOSE 8080

CMD ["/app/lamp-api"]
