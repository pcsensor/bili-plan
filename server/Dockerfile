FROM rust:latest AS builder
WORKDIR /usr/src/app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates tzdata && rm -rf /var/lib/apt/lists/*
ENV TZ=Asia/Shanghai
WORKDIR /app
COPY --from=builder /usr/src/app/target/release/bili-plan-server /app/bili-plan-server
EXPOSE 3005
CMD ["/app/bili-plan-server"]
