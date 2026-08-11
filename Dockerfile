# musl 정적 빌드 → distroless static(nonroot). Rust 타깃 triple은 TARGETARCH에서 파생한다.
#
# ⚠️ `--platform=$BUILDPLATFORM` — 빌드 스테이지를 **러너 네이티브 아키텍처**에 고정한다.
#   upstream reusable-app-build.yaml이 멀티아치가 되면 amd64 leg는 QEMU를 타는데, Rust release
#   빌드는 이 레포에서 가장 무거운 작업이라 에뮬레이션 비용이 그대로 CI 시간이 된다.
#   빌드 스테이지를 네이티브에 고정하고 타깃만 크로스컴파일하면 에뮬레이션을 아예 안 탄다.
#   실측(arm64 호스트): 크로스 62s vs 에뮬레이션 145s. CI의 QEMU는 격차가 더 크다.
#   최종 이미지는 distroless(타깃 arch) + 정적 musl 바이너리라 결과물은 여전히 진짜 amd64다.
FROM --platform=$BUILDPLATFORM rust:1.93-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# ⚠️ TARGETARCH에 기본값을 주지 말 것 — stage-level ARG 기본값은 BuildKit predefined platform arg를
#   이겨서, 잘못된 아키텍처 바이너리를 담은 이미지를 조용히 만든다. 비워두면 아래 case가 exit 1로 막는다.
# COPY --from은 셸 변수를 못 쓰므로 빌드 스테이지 안에서 고정 경로(/app/files)로 옮겨둔다.
ARG TARGETARCH
ARG BUILDARCH
# 크로스일 때만 링커를 바꾼다(RUSTFLAGS). alpine의 cc는 호스트 arch 전용이라 타깃이 다르면 링크가 깨진다.
# 이 크레이트는 의존성이 전부 순수 Rust다(ring/openssl/cc 빌드스크립트 0건, libc·linux-raw-sys는 FFI
# 선언뿐) — C 툴체인이 필요 없어 rust-lld만으로 정적 musl 링킹이 끝난다.
# ⚠️ 네이티브(TARGETARCH == BUILDARCH)에선 이 분기를 타지 않는다. 링커가 그대로라 산출물이 변경 전과
#   **바이트 동일**하다(실측: 양쪽 sha256 b485ed16f4f7f1be 일치). 그게 이 커밋의 no-op 근거다.
# ⚠️ 주석을 RUN 연속행 **안**에 두지 말 것 — Dockerfile 파서 버전에 따라 뒤 내용이 통째로 삼켜진다.
RUN set -eu; \
    case "$TARGETARCH" in \
      arm64) TRIPLE=aarch64-unknown-linux-musl ;; \
      amd64) TRIPLE=x86_64-unknown-linux-musl  ;; \
      *) echo "지원하지 않는 TARGETARCH='$TARGETARCH' — arm64|amd64만 지원" >&2; exit 1 ;; \
    esac; \
    rustup target add "$TRIPLE"; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then export RUSTFLAGS="-C linker=rust-lld"; fi; \
    cargo build --release --target "$TRIPLE"; \
    cp "/app/target/$TRIPLE/release/files" /app/files

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /app/files /files
USER nonroot
EXPOSE 8080 8081
ENTRYPOINT ["/files"]
