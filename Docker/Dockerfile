FROM rust:1.98.0-bookworm AS builder

RUN rustup toolchain install 1.98.1 --profile minimal
WORKDIR /src
COPY RustBot/ ./
ARG VERSION=1.0.0
ARG GIT_COMMIT=unknown
ARG BUILD_TIME=unknown
RUN TGBOT_VERSION="$VERSION" \
    TGBOT_GIT_COMMIT="$GIT_COMMIT" \
    TGBOT_BUILD_TIME="$BUILD_TIME" \
    cargo +1.98.1 build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /root
COPY --from=builder /src/target/release/TGBot_RSS /app/TGBot_RSS
COPY RustBot/config.json /app/config.json
COPY Docker/main.sh /app/main.sh
RUN chmod +x /app/TGBot_RSS /app/main.sh

ENV TZ=Asia/Shanghai
ENTRYPOINT ["/app/main.sh"]
