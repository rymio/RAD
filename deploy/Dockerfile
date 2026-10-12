# syntax=docker/dockerfile:1.7
# RAD server — build:  docker build -f deploy/Dockerfile -t rad-server .
FROM rust:1-bookworm AS build
# llama.cpp (built-in embedding engine) is compiled from source: cmake + clang/libclang.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libclang-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p rad-server --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates ffmpeg tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data rad \
 && mkdir -p /data && chown rad /data
COPY --from=build /src/target/release/rad-server /usr/local/bin/rad-server
# Built-in embedding model, verified, so semantic search works offline from the first start.
ADD --checksum=sha256:3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7 \
    https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q8_0.gguf \
    /opt/rad/models/nomic-embed-text-v1.5.Q8_0.gguf
RUN chmod 644 /opt/rad/models/*.gguf
USER rad
VOLUME /data
ENV RAD_VAULT=/data/personal.radvault \
    RAD_BUNDLED_MODELS_DIR=/opt/rad/models \
    RAD_LISTEN=0.0.0.0:8765 \
    RAD_NAME="RAD server"
EXPOSE 8765
ENTRYPOINT ["/usr/bin/tini", "--", "rad-server"]
