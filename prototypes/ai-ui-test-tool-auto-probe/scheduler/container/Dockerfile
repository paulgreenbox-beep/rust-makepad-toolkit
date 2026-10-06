# syntax=docker/dockerfile:1.7
FROM rust:1.94.0-bookworm AS builder

ARG DEBIAN_FRONTEND=noninteractive
ARG MAKEPAD_REVISION
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    clang \
    libasound2-dev \
    libdrm-dev \
    libegl1-mesa-dev \
    libgbm-dev \
    libgl1-mesa-dev \
    libgles2-mesa-dev \
    libglx-dev \
    libpulse-dev \
    libssl-dev \
    libwayland-dev \
    libx11-dev \
    libxcursor-dev \
    libxi-dev \
    libxinerama-dev \
    libxkbcommon-dev \
    libxrandr-dev \
    pkg-config \
    wayland-protocols \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY examples ./examples
COPY prototypes/ai-ui-test-tool ./prototypes/ai-ui-test-tool
COPY prototypes/ai-ui-test-tool-auto-probe ./prototypes/ai-ui-test-tool-auto-probe
COPY --from=makepad-src . ./.deps/makepad

RUN cargo build --locked --release \
    --manifest-path prototypes/ai-ui-test-tool-auto-probe/Cargo.toml \
    --bin ai-ui-test

FROM debian:bookworm-slim AS runtime

ARG DEBIAN_FRONTEND=noninteractive
ARG MAKEPAD_REVISION
LABEL org.opencontainers.image.title="AI UI Test Tool container runner" org.opencontainers.image.makepad-revision="${MAKEPAD_REVISION}"
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libasound2 \
    libdrm2 \
    libegl1 \
    libgbm1 \
    libgl1 \
    libpulse0 \
    libwayland-client0 \
    libx11-6 \
    libxcursor1 \
    libxi6 \
    libxinerama1 \
    libxkbcommon0 \
    libxrandr2 \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 10001 aiui \
 && useradd --uid 10001 --gid aiui --home-dir /home/ai-ui --create-home aiui

COPY --from=builder /workspace/prototypes/ai-ui-test-tool-auto-probe/target/release/ai-ui-test /opt/ai-ui/ai-ui-test

USER 10001:10001
WORKDIR /work
ENTRYPOINT ["/opt/ai-ui/ai-ui-test"]
