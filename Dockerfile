# syntax=docker/dockerfile:1

FROM rust:1.94-bookworm AS build
WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 app \
    && useradd --uid 10001 --gid app --home-dir /app --shell /usr/sbin/nologin --no-create-home app

WORKDIR /app

COPY --from=build /build/target/release/cyder-mcp-template /usr/local/bin/cyder-mcp-template

EXPOSE 8000

USER app

CMD ["cyder-mcp-template", "streamhttp", "--host", "0.0.0.0", "--port", "8000"]
