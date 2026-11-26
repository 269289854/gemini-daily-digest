# syntax=docker/dockerfile:1

FROM node:22-bookworm-slim AS web-builder
WORKDIR /app

ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable && corepack prepare pnpm@12.21.0 --activate

COPY package.json pnpm-workspace.yaml pnpm-lock.yaml ./
COPY apps/web/package.json ./apps/web/
RUN pnpm install --frozen-lockfile --filter @gemini-daily-digest/web...

COPY apps/web ./apps/web
RUN pnpm --dir apps/web run build

FROM rust:1-bookworm AS rust-builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p digest-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 digest \
    && useradd --system --uid 10001 --gid digest --no-create-home --shell /usr/sbin/nologin digest

WORKDIR /app
COPY --from=rust-builder --chown=digest:digest /app/target/release/digest-server /app/digest-server
COPY --from=web-builder --chown=digest:digest /app/apps/web/dist /app/web/dist

USER digest

ENV SERVER_BIND_ADDR=0.0.0.0:8080
ENV STATIC_DIR=/app/web/dist
ENV RUST_LOG=info

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["curl", "-fsS", "http://127.0.0.1:8080/health"]

ENTRYPOINT ["/app/digest-server"]
