FROM rust:1-slim AS build

WORKDIR /workspace

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release

FROM debian:trixie-slim

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 appuser \
    && mkdir -p /state \
    && chown -R appuser:appuser /state

COPY --from=build /workspace/target/release/mailbox-viewer /app/mailbox-viewer

ENV STATE_DB_PATH=/state/mailbox-viewer.db
ENV HTTP_PORT=8080

EXPOSE 8080

USER appuser

ENTRYPOINT ["/app/mailbox-viewer"]
