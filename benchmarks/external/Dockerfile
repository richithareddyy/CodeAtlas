# The pinned toolchain image plus git, which ingestion uses to read
# repository metadata. Built once; probes run from it with no network.
FROM rust:1.99-slim-bookworm@sha256:f2c004e3d2d8b547e8cf7ffbc29205385f4e71d23e53e4e383065db5e9f822cd
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN git config --system --add safe.directory '*'
ENV CARGO_HOME=/cargo-home
