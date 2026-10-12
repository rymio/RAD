# RAD server — build:  docker build -f deploy/Dockerfile -t rad-server .
FROM rust:1-bookworm AS build
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
USER rad
VOLUME /data
ENV RAD_VAULT=/data/personal.radvault \
    RAD_LISTEN=0.0.0.0:8765 \
    RAD_NAME="RAD server"
EXPOSE 8765
ENTRYPOINT ["/usr/bin/tini", "--", "rad-server"]
