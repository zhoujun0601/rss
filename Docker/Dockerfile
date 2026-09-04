FROM golang:1.27.1-alpine AS builder

RUN apk add --no-cache gcc musl-dev
WORKDIR /src

COPY TGRSSBot/go.mod TGRSSBot/go.sum ./
RUN go mod download

COPY TGRSSBot/ ./
RUN CGO_ENABLED=1 GOOS=linux go build -trimpath -o /out/TGBot_RSS .

FROM frolvlad/alpine-glibc:latest
RUN apk add --no-cache bash jq
WORKDIR /root

COPY --from=builder /out/TGBot_RSS /app/TGBot_RSS
COPY TGRSSBot/config.json /app/config.json
COPY Docker/main.sh /app/main.sh
RUN chmod +x /app/TGBot_RSS /app/main.sh
ENV TZ=Asia/Shanghai
ENTRYPOINT ["/app/main.sh"]
