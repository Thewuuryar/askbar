# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# check=skip=SecretsUsedInArgOrEnv
#
# One target per service:
#   docker build --target server     -t askbar-server .
#   docker build --target bootstrap  -t askbar-bootstrap .
#   docker build --target auditor    -t askbar-auditor .
#   docker build --target model-gateway -t askbar-model-gateway .
#   docker build --target alerter    -t askbar-alerter .
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
COPY crates/askbar-audit/Cargo.toml crates/askbar-audit/
COPY services/server/Cargo.toml services/server/
COPY services/admin/Cargo.toml services/admin/
COPY services/cpa-supervisor/Cargo.toml services/cpa-supervisor/
COPY services/bootstrap/Cargo.toml services/bootstrap/
COPY services/auditor/Cargo.toml services/auditor/
COPY services/model-gateway/Cargo.toml services/model-gateway/
COPY services/alerter/Cargo.toml services/alerter/
RUN for d in crates/* services/*; do mkdir -p "$d/src" && touch "$d/src/lib.rs" && echo 'fn main() {}' > "$d/src/main.rs"; done \
 && cargo build --release --locked -p askbar-server -p askbar-admin -p askbar-cpa-supervisor -p askbar-bootstrap -p askbar-auditor -p askbar-model-gateway -p askbar-alerter \
 && rm -rf crates/*/src services/*/src target/release/.fingerprint/askbar-*
# The real sources. Removing the members' fingerprints above makes cargo rebuild them, whatever the
# copied files' timestamps.
COPY crates crates
COPY services services
# The desktop client is its own workspace (clients/desktop) and is not part of any image.
RUN cargo build --release --locked -p askbar-server -p askbar-admin -p askbar-cpa-supervisor -p askbar-bootstrap -p askbar-auditor -p askbar-model-gateway -p askbar-alerter

# --------------------------------------------------------------- bootstrap
# Creates the per-link tokens and exits (ADR 0049). Runs as root with CHOWN as its only capability, no
# network, and mounts the credential volumes only. No user is made: it must own a directory it writes
# to, then give it to the service that reads it.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS bootstrap
RUN mkdir /server-token /admin-token /auditor-token /model-gateway-token /alerter-token
COPY --from=build /src/target/release/askbar-bootstrap /usr/local/bin/askbar-bootstrap
ENTRYPOINT ["askbar-bootstrap"]

# ------------------------------------------------------------------ admin
# The admin service: the admin site and its own database. No TCP port: it listens on a unix
# socket that only Caddy is given. Runs as uid 10001.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS admin
RUN groupadd --system --gid 10001 askbar-admin && useradd --system --uid 10001 --gid 10001 --home-dir /admin-data --shell /usr/sbin/nologin askbar-admin \
 && mkdir /admin-data /admin-secrets /admin-token /cpa-config /caddy-secrets /run/admin /caddy-run /backup \
 && chown askbar-admin:askbar-admin /admin-data /admin-secrets /admin-token /cpa-config /caddy-secrets /run/admin /caddy-run /backup \
 && chmod 700 /admin-secrets \
 && chmod 770 /caddy-run
COPY --from=build /src/target/release/askbar-admin /usr/local/bin/askbar-admin
USER askbar-admin
ENV ASKBAR_ADMIN_DATA_DIR=/admin-data \
    ASKBAR_ADMIN_SECRETS_DIR=/admin-secrets \
    ASKBAR_ADMIN_SOCKET=/run/admin/admin.sock \
    ASKBAR_ADMIN_TOKEN_DIR=/admin-token \
    ASKBAR_CPA_CONFIG_DIR=/cpa-config \
    ASKBAR_CADDY_SOCKET=/caddy-run/admin.sock \
    ASKBAR_CADDY_SECRETS_DIR=/caddy-secrets \
    ASKBAR_BACKUP_DIR=/backup
VOLUME /admin-data /admin-secrets /backup
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

# ----------------------------------------------------------------- auditor
# The audit store as its own container (ADR 0051): /api/audit/v1 on 8083, on `audit-*` networks only, with no route
# out. Runs as uid 10003 from its first instruction, with no capabilities. /audit belongs to that uid in the image,
# so Docker gives a new `auditor-data` volume the same owner. `df` (coreutils) tells it how much room is left.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS auditor
RUN groupadd --system --gid 10003 askbar-auditor && useradd --system --uid 10003 --gid 10003 --home-dir /audit --shell /usr/sbin/nologin askbar-auditor \
 && mkdir /audit /auditor-token && chown askbar-auditor:askbar-auditor /audit /auditor-token
COPY --from=build /src/target/release/askbar-auditor /usr/local/bin/askbar-auditor
USER askbar-auditor
ENV ASKBAR_AUDIT_DIR=/audit \
    ASKBAR_AUDITOR_TOKEN_DIR=/auditor-token \
    ASKBAR_AUDIT_API_ADDR=0.0.0.0:8083
VOLUME /audit
EXPOSE 8083
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-auditor", "healthcheck"]
ENTRYPOINT ["askbar-auditor"]

# ------------------------------------------------------------- model-gateway
# Provider calls and provider keys (ADR 0054): /api/model-gateway/v1 on two listeners, one for the server and one for
# the admin, each on the address its network alias gives (compose sets them). Runs as uid 10004 from its first
# instruction, with no capabilities, and has no data volume: the keys are in memory only. Its one volume is the
# credential volume `model-gateway-token`, mounted read-only.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS model-gateway
RUN groupadd --system --gid 10004 askbar-gateway && useradd --system --uid 10004 --gid 10004 --home-dir /nonexistent --shell /usr/sbin/nologin askbar-gateway \
 && mkdir /model-gateway-token && chown askbar-gateway:askbar-gateway /model-gateway-token
COPY --from=build /src/target/release/askbar-model-gateway /usr/local/bin/askbar-model-gateway
USER askbar-gateway
ENV ASKBAR_GATEWAY_TOKEN_DIR=/model-gateway-token
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-model-gateway", "healthcheck"]
ENTRYPOINT ["askbar-model-gateway"]

# ----------------------------------------------------------------- alerter
# Alert delivery (ADR 0055): /api/alerter/v1 on two listeners, one for the auditor and one for the admin, each on the address its
# network alias gives (compose sets them). Runs as uid 10005 from its first instruction, with no capabilities. Two volumes: its
# data volume `alerter-data` (the repeat ledger: keys, times and counts, no secret and no content), and the credential volume
# `alerter-token`, mounted read-only. The delivery settings are the admin's and held in memory only. Docker makes a new
# `alerter-data` volume owned by uid 10005, as /alerter-data is here.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS alerter
RUN groupadd --system --gid 10005 askbar-alerter && useradd --system --uid 10005 --gid 10005 --home-dir /alerter-data --shell /usr/sbin/nologin askbar-alerter \
 && mkdir /alerter-data /alerter-token && chown askbar-alerter:askbar-alerter /alerter-data /alerter-token
COPY --from=build /src/target/release/askbar-alerter /usr/local/bin/askbar-alerter
USER askbar-alerter
ENV ASKBAR_ALERTER_TOKEN_DIR=/alerter-token ASKBAR_ALERTER_DATA_DIR=/alerter-data
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-alerter", "healthcheck"]
ENTRYPOINT ["askbar-alerter"]

# ----------------------------------------------------------------- server
# The server: the client API on 8080 and the server API on its own address. Runs as uid 10002,
# so it shares no file ownership with the admin.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS server
RUN groupadd --system --gid 10002 askbar && useradd --system --uid 10002 --gid 10002 --home-dir /data --shell /usr/sbin/nologin askbar \
 && mkdir /data && chown askbar:askbar /data
COPY --from=build /src/target/release/askbar-server /usr/local/bin/askbar-server
USER askbar
ENV ASKBAR_DATA_DIR=/data \
    ASKBAR_CLIENT_API_ADDR=0.0.0.0:8080 \
    ASKBAR_SERVER_TOKEN_DIR=/server-token \
    ASKBAR_SERVER_TOKEN_HASH_FILE=/server-token/accept/admin-server.sha256
# ASKBAR_SERVER_API_ADDR has no default: it must be one specific address (the compose file sets it).
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-server", "healthcheck"]
ENTRYPOINT ["askbar-server"]
