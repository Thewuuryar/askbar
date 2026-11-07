# syntax=docker/dockerfile:1
#
# One target per service:
#   docker build --target server     -t askbar-server .
#   docker build --target admin      -t askbar-admin  .
#   docker build --target caddy      -t askbar-caddy  .
#   docker build --target cpa        -t askbar-cpa    .
# A plain `docker build .` builds the last stage, the server.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY services services
# The desktop client is its own workspace (clients/desktop) and is not part of any image.
RUN cargo build --release -p askbar-server -p askbar-admin -p askbar-cpa-supervisor

# ------------------------------------------------------------------ admin
# The admin service: the admin site and its own database. No TCP port: it listens on a unix
# socket that only Caddy is given. Runs as uid 10001.
FROM debian:bookworm-slim AS admin
RUN groupadd --system --gid 10001 askbar-admin && useradd --system --uid 10001 --gid 10001 --home-dir /admin-data --shell /usr/sbin/nologin askbar-admin \
 && mkdir /admin-data /admin-secrets /server-token /cpa-config /caddy-secrets /run/admin /caddy-run \
 && chown askbar-admin:askbar-admin /admin-data /admin-secrets /server-token /cpa-config /caddy-secrets /run/admin /caddy-run \
 && chmod 700 /admin-secrets \
 && chmod 770 /caddy-run
COPY --from=build /src/target/release/askbar-admin /usr/local/bin/askbar-admin
USER askbar-admin
ENV ASKBAR_ADMIN_DATA_DIR=/admin-data \
    ASKBAR_ADMIN_SECRETS_DIR=/admin-secrets \
    ASKBAR_ADMIN_SOCKET=/run/admin/admin.sock \
    ASKBAR_SERVER_TOKEN_DIR=/server-token \
    ASKBAR_CPA_CONFIG_DIR=/cpa-config \
    ASKBAR_CADDY_SOCKET=/caddy-run/admin.sock \
    ASKBAR_CADDY_SECRETS_DIR=/caddy-secrets
VOLUME /admin-data /admin-secrets
# Healthy when the admin socket answers; there is no port to probe.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-admin", "healthcheck"]
ENTRYPOINT ["askbar-admin"]

# ------------------------------------------------------------------ caddy
# Caddy with the Cloudflare DNS plugin, used to get certificates without opening port 80/443.
FROM caddy:2-builder AS caddy-build
RUN xcaddy build --with github.com/caddy-dns/cloudflare

FROM caddy:2 AS caddy
COPY --from=caddy-build /usr/bin/caddy /usr/bin/caddy
COPY deploy/caddy/bootstrap.Caddyfile /etc/caddy/bootstrap.Caddyfile
# The admin service pushes the real configuration through the admin socket; `--resume` brings back
# the last one after a restart. The bootstrap file only moves Caddy's admin API onto a unix socket.
CMD ["caddy", "run", "--config", "/etc/caddy/bootstrap.Caddyfile", "--adapter", "caddyfile", "--resume"]

# -------------------------------------------------------------------- cpa
# CLIProxyAPI, pinned by digest, with our supervisor as the entry point (ADR 0037). The supervisor runs it as a
# child and restarts it when /cpa-config/config.yaml changes, when the admin writes a new value to
# /cpa-config/restart-request, or after a crash; it opens no port. CLIProxyAPI itself is unchanged.
FROM eceasy/cli-proxy-api:v8.0.51@sha256:15d13889e1a39a688be306dfbf2f6cf9003e99cd913b5070fa463f8ab2aa0031 AS cpa
COPY --from=build /src/target/release/askbar-cpa-supervisor /usr/local/bin/askbar-cpa-supervisor
WORKDIR /CLIProxyAPI
ENV ASKBAR_CPA_CONFIG_DIR=/cpa-config
ENTRYPOINT ["askbar-cpa-supervisor"]
CMD ["./CLIProxyAPI", "-config", "/cpa-config/config.yaml"]

# ----------------------------------------------------------------- server
# The server: the client API on 8080 and the server API on its own address. Runs as uid 10002,
# so it shares no file ownership with the admin.
FROM debian:bookworm-slim AS server
RUN groupadd --system --gid 10002 askbar && useradd --system --uid 10002 --gid 10002 --home-dir /data --shell /usr/sbin/nologin askbar \
 && mkdir /data /secrets /audit && chown askbar:askbar /data /secrets /audit
COPY --from=build /src/target/release/askbar-server /usr/local/bin/askbar-server
USER askbar
ENV ASKBAR_DATA_DIR=/data \
    ASKBAR_SECRETS_DIR=/secrets \
    ASKBAR_AUDIT_DIR=/audit \
    ASKBAR_CLIENT_API_ADDR=0.0.0.0:8080 \
    ASKBAR_SERVER_TOKEN_HASH_FILE=/server-token/token.sha256
# ASKBAR_SERVER_API_ADDR has no default: it must be one specific address (the compose file sets it).
VOLUME /data /secrets /audit
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-server", "healthcheck"]
ENTRYPOINT ["askbar-server"]
