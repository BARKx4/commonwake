FROM rust:1.96-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake git perl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY . .
RUN set -eu; \
    mkdir -p .commonwake-build; \
    if [ ! -s .commonwake-build/commonwake.bundle ]; then \
        git init -b main; \
        git config user.name Commonwake; \
        git config user.email source-capsule@commonwake.invalid; \
        git add --all; \
        GIT_AUTHOR_DATE=2000-01-01T00:00:00Z \
        GIT_COMMITTER_DATE=2000-01-01T00:00:00Z \
            git commit -m 'Exact Docker build-context source snapshot'; \
        git bundle create .commonwake-build/commonwake.bundle \
            HEAD refs/heads/main --tags; \
        git rev-parse HEAD > .commonwake-build/revision; \
        printf '%s\n' true > .commonwake-build/exact; \
        printf '%s\n' build-context-snapshot > .commonwake-build/provenance; \
        printf '%s\n' refs/heads/main > .commonwake-build/default-ref; \
    fi; \
    git bundle list-heads .commonwake-build/commonwake.bundle >/dev/null
ENV COMMONWAKE_SOURCE_BUNDLE=/source/.commonwake-build/commonwake.bundle
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
