# syntax=docker/dockerfile:1
#
# Two images from one build:
#   docker build --target server -t askbar-server .
#   docker build --target admin  -t askbar-admin  .
# A plain `docker build .` builds the last stage, the server.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY askbar-proto askbar-proto
COPY askbar-server askbar-server
COPY askbar-admin askbar-admin
# The desktop client (src-tauri) is not part of either image.
RUN sed -i 's/^members = .*/members = ["askbar-proto", "askbar-server", "askbar-admin"]/' Cargo.toml \
 && cargo build --release -p askbar-server -p askbar-admin

# ------------------------------------------------------------------ admin
# The admin service: the admin site and its own database. No TCP port: it listens on a unix
# socket that only Caddy is given. Runs as uid 10001.
FROM debian:bookworm-slim AS admin
RUN groupadd --system --gid 10001 askbar-admin && useradd --system --uid 10001 --gid 10001 --home-dir /admin-data --shell /usr/sbin/nologin askbar-admin \
 && mkdir /admin-data /admin-secrets /mgmt /caddy-secrets /run/admin /caddy-run \
 && chown askbar-admin:askbar-admin /admin-data /admin-secrets /mgmt /caddy-secrets /run/admin /caddy-run \
 && chmod 700 /admin-secrets
COPY --from=build /src/target/release/askbar-admin /usr/local/bin/askbar-admin
USER askbar-admin
ENV ASKBAR_ADMIN_DATA_DIR=/admin-data \
    ASKBAR_ADMIN_SECRETS_DIR=/admin-secrets \
    ASKBAR_ADMIN_SOCKET=/run/admin/admin.sock \
    ASKBAR_MGMT_DIR=/mgmt \
    ASKBAR_CADDY_SOCKET=/caddy-run/admin.sock \
    ASKBAR_CADDY_SECRETS_DIR=/caddy-secrets
VOLUME /admin-data /admin-secrets
# Healthy when the admin socket answers; there is no port to probe.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-admin", "healthcheck"]
ENTRYPOINT ["askbar-admin"]

# ----------------------------------------------------------------- server
# The server: the device API on 8080 and the management API on its own address. Runs as uid 10002,
# so it shares no file ownership with the admin.
FROM debian:bookworm-slim AS server
RUN groupadd --system --gid 10002 askbar && useradd --system --uid 10002 --gid 10002 --home-dir /data --shell /usr/sbin/nologin askbar \
 && mkdir /data /secrets && chown askbar:askbar /data /secrets
COPY --from=build /src/target/release/askbar-server /usr/local/bin/askbar-server
USER askbar
ENV ASKBAR_DATA_DIR=/data \
    ASKBAR_SECRETS_DIR=/secrets \
    ASKBAR_API_ADDR=0.0.0.0:8080 \
    ASKBAR_MGMT_TOKEN_HASH_FILE=/mgmt/token.sha256
# ASKBAR_MGMT_ADDR has no default: it must be one specific address (the compose file sets it).
VOLUME /data /secrets
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-server", "healthcheck"]
ENTRYPOINT ["askbar-server"]
