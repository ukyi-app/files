# musl 정적 빌드 → distroless static(nonroot). Rust 타깃 triple은 TARGETARCH에서 파생한다.
FROM rust:1.93-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# ⚠️ TARGETARCH에 기본값을 주지 말 것 — stage-level ARG 기본값은 BuildKit predefined platform arg를
#   이겨서, 잘못된 아키텍처 바이너리를 담은 이미지를 조용히 만든다. 비워두면 아래 case가 exit 1로 막는다.
# COPY --from은 셸 변수를 못 쓰므로 빌드 스테이지 안에서 고정 경로(/app/files)로 옮겨둔다.
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      arm64) TRIPLE=aarch64-unknown-linux-musl ;; \
      amd64) TRIPLE=x86_64-unknown-linux-musl  ;; \
      *) echo "지원하지 않는 TARGETARCH='$TARGETARCH' — arm64|amd64만 지원" >&2; exit 1 ;; \
    esac && \
    rustup target add "$TRIPLE" && \
    cargo build --release --target "$TRIPLE" && \
    cp "/app/target/$TRIPLE/release/files" /app/files

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /app/files /files
USER nonroot
EXPOSE 8080 8081
ENTRYPOINT ["/files"]
