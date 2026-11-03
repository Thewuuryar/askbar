# syntax=docker/dockerfile:1
#
# Two images from one build:
#   docker build --target server -t askbar-server .
#   docker build --target admin  -t askbar-admin  .
#   docker build --target controller -t askbar-controller .
# A plain `docker build .` builds the last stage, the server.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY askbar-proto askbar-proto
COPY askbar-server askbar-server
COPY askbar-admin askbar-admin
COPY askbar-controller askbar-controller
# The desktop client (src-tauri) is not part of either image.
RUN sed -i 's/^members = .*/members = ["askbar-proto", "askbar-server", "askbar-admin", "askbar-controller"]/' Cargo.toml \
 && cargo build --release -p askbar-server -p askbar-admin -p askbar-controller

# ------------------------------------------------------------------ admin
# The admin service: the admin site and its own database. No TCP port: it listens on a unix
# socket that only Caddy is given. Runs as uid 10001.
FROM debian:bookworm-slim AS admin
RUN groupadd --system --gid 10001 askbar-admin && useradd --system --uid 10001 --gid 10001 --home-dir /admin-data --shell /usr/sbin/nologin askbar-admin \
 && mkdir /admin-data /admin-secrets /server-token /control-token /caddy-secrets /run/admin /caddy-run \
 && chown askbar-admin:askbar-admin /admin-data /admin-secrets /server-token /control-token /caddy-secrets /run/admin /caddy-run \
 && chmod 700 /admin-secrets
COPY --from=build /src/target/release/askbar-admin /usr/local/bin/askbar-admin
USER askbar-admin
ENV ASKBAR_ADMIN_DATA_DIR=/admin-data \
    ASKBAR_ADMIN_SECRETS_DIR=/admin-secrets \
    ASKBAR_ADMIN_SOCKET=/run/admin/admin.sock \
    ASKBAR_SERVER_TOKEN_DIR=/server-token \
    ASKBAR_CONTROL_TOKEN_DIR=/control-token \
    ASKBAR_CADDY_SOCKET=/caddy-run/admin.sock \
    ASKBAR_CADDY_SECRETS_DIR=/caddy-secrets
VOLUME /admin-data /admin-secrets
# Healthy when the admin socket answers; there is no port to probe.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-admin", "healthcheck"]
ENTRYPOINT ["askbar-admin"]

# ------------------------------------------------------------- controller
# The only image that is given the Docker socket. It starts, stops and restarts the server, Caddy and
# CLIProxyAPI for the admin and can do nothing else (see askbar-controller/src/lib.rs). It runs as root
# only so that it can open the socket, whatever group owns it; compose drops every capability.
FROM debian:bookworm-slim AS controller
# The token-hash volume is owned by the admin's uid (10001), whichever container Docker creates it for first.
RUN mkdir /control-token && chown 10001:10001 /control-token
COPY --from=build /src/target/release/askbar-controller /usr/local/bin/askbar-controller
ENV ASKBAR_CONTROL_TOKEN_HASH_FILE=/control-token/token.sha256 \
    ASKBAR_DOCKER_SOCKET=/var/run/docker.sock
# ASKBAR_CONTROL_ADDR has no default: it must be one specific address (the compose file sets it).
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-controller", "healthcheck"]
ENTRYPOINT ["askbar-controller"]

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
