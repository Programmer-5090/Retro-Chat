FROM rust:latest AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin server

FROM debian:trixie-slim
WORKDIR /usr/local/bin
COPY --from=builder /app/target/release/server .
CMD ["./server"]
