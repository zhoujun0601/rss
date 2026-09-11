FROM rust:1.98.0-bookworm AS builder

RUN rustup toolchain install 1.98.1 --profile minimal
WORKDIR /src
COPY RustBot/ ./
ARG VERSION=1.0.0
ARG GIT_COMMIT=unknown
ARG BUILD_TIME=unknown
RUN RSSBOT_VERSION="$VERSION" \
    RSSBOT_GIT_COMMIT="$GIT_COMMIT" \
    RSSBOT_BUILD_TIME="$BUILD_TIME" \
    cargo +1.98.1 build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates gosu tzdata \
    && groupadd --gid 10001 tgbot \
    && useradd --uid 10001 --gid tgbot --home-dir /data --no-create-home --shell /usr/sbin/nologin tgbot \
    && mkdir -p /data \
    && chown tgbot:tgbot /data \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /data
COPY --from=builder /src/target/release/RSSBOT /app/RSSBOT
COPY RustBot/config.json /app/config.json
COPY Docker/main.sh /app/main.sh
RUN chmod +x /app/RSSBOT /app/main.sh

ENV TZ=Asia/Shanghai
ENTRYPOINT ["/app/main.sh"]
