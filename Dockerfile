# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY core core
COPY askbar-proto askbar-proto
COPY askbar-server askbar-server
# The desktop client (src-tauri) is not part of the server image.
RUN sed -i 's/^members = .*/members = ["core", "askbar-proto", "askbar-server"]/' Cargo.toml \
 && cargo build --release -p askbar-server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin askbar \
 && mkdir /data /secrets && chown askbar:askbar /data /secrets
COPY --from=build /src/target/release/askbar-server /usr/local/bin/askbar-server
USER askbar
ENV ASKBAR_DATA_DIR=/data \
    ASKBAR_SECRETS_DIR=/secrets \
    ASKBAR_API_ADDR=0.0.0.0:8080 \
    ASKBAR_ADMIN_ADDR=0.0.0.0:8081
VOLUME /data /secrets
EXPOSE 8080 8081
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["askbar-server", "healthcheck"]
ENTRYPOINT ["askbar-server"]
