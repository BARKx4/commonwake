FROM rust:1.96-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake perl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY docs ./docs
RUN cargo build --locked --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 commonwake \
    && install -d -o commonwake -g commonwake /data
COPY --from=builder /source/target/release/commonwake /usr/local/bin/commonwake

USER commonwake
VOLUME ["/data"]
EXPOSE 8787 8080 8443
ENV COMMONWAKE_DATA_DIR=/data
ENV COMMONWAKE_BIND=0.0.0.0:8787
ENTRYPOINT ["commonwake"]
CMD ["join"]
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:8787/v1/health >/dev/null || exit 1
