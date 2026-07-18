FROM rust:latest AS builder
RUN apt-get update && apt-get install -y libasound2-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release --bin server

FROM debian:trixie-slim
WORKDIR /usr/local/bin
COPY --from=builder /app/target/release/server .
CMD ["./server"]
