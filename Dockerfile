# musl 정적 빌드 → distroless static(nonroot). arm64(aarch64) 네이티브.
FROM rust:1.93-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN rustup target add aarch64-unknown-linux-musl && \
    cargo build --release --target aarch64-unknown-linux-musl

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /app/target/aarch64-unknown-linux-musl/release/files /files
USER nonroot
EXPOSE 8080 8081
ENTRYPOINT ["/files"]
