# Build context = repository root:   docker build -f deploy/Dockerfile -t rajeshdb-platform .
# NOTE: compiling Rust needs RAM (about 2-4 GB). On a small VPS add swap, or build the image
# in GitHub Actions (.github/workflows/docker-publish.yml) and pull it instead.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY engine/ engine/
WORKDIR /src/engine
RUN cargo build --release -p rajeshdb-platform

FROM debian:bookworm-slim
# Unprivileged user; the only writable place is /data (the persistent volume).
RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin rajeshdb \
 && mkdir -p /data && chown 10001:10001 /data
COPY --from=build /src/engine/target/release/rajeshdb-platform /usr/local/bin/rajeshdb-platform
USER 10001:10001
ENV RAJESHDB_DATA_DIR=/data \
    RAJESHDB_BIND=0.0.0.0:8081
VOLUME ["/data"]
EXPOSE 8081
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["rajeshdb-platform", "healthcheck"]
CMD ["rajeshdb-platform", "serve"]
