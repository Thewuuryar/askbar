# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# check=skip=SecretsUsedInArgOrEnv
#
# One target per service:
#   docker build --target server     -t askbar-server .
#   docker build --target admin      -t askbar-admin  .
#   docker build --target caddy      -t askbar-caddy  .
#   docker build --target cpa        -t askbar-cpa    .
# A plain `docker build .` builds the last stage, the server.
# The build check SecretsUsedInArgOrEnv is skipped: it flags variable names with TOKEN or SECRET in them,
# and here every one holds a path (AGENTS.md: an environment variable holds a path or an address,
# never a secret).
# Every image is pinned by digest, and the Caddy plugin by version (ADR 0017); the tag beside each
# digest says which line it follows. To update one, take the tag's current digest and test the stack.

FROM rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# Dependencies first, in a layer of their own: it is rebuilt only when a manifest or Cargo.lock changes,
# not on every source change. Each member gets stub sources; a new workspace member needs its line here.
COPY crates/askbar-proto/Cargo.toml crates/askbar-proto/
COPY crates/askbar-service/Cargo.toml crates/askbar-service/
COPY services/server/Cargo.toml services/server/
COPY services/admin/Cargo.toml services/admin/
COPY services/cpa-supervisor/Cargo.toml services/cpa-supervisor/
RUN for d in crates/* services/*; do mkdir -p "$d/src" && touch "$d/src/lib.rs" && echo 'fn main() {}' > "$d/src/main.rs"; done \
 && cargo build --release --locked -p askbar-server -p askbar-admin -p askbar-cpa-supervisor \
 && rm -rf crates/*/src services/*/src target/release/.fingerprint/askbar-*
# The real sources. Removing the members' fingerprints above makes cargo rebuild them, whatever the
# copied files' timestamps.
COPY crates crates
COPY services services
# The desktop client is its own workspace (clients/desktop) and is not part of any image.
RUN cargo build --release --locked -p askbar-server -p askbar-admin -p askbar-cpa-supervisor

# ------------------------------------------------------------------ admin
# The admin service: the admin site and its own database. No TCP port: it listens on a unix
# socket that only Caddy is given. Runs as uid 10001.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS admin
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
# The builder's digest fixes the Caddy version it builds (v2.11.9); the plugin is pinned to a release.
FROM caddy:2-builder@sha256:3db11aead97f1fdff9c563d0dd8e7fba2e4c7031bef40f3e44fd1982f3e8366b AS caddy-build
RUN xcaddy build --with github.com/caddy-dns/cloudflare@v0.2.4

FROM caddy:2@sha256:ce773ec7a742ce15ee6bf2bb5f65882b9e2a4487b10733e6f1484f86e6b5d8bc AS caddy
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
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS server
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
