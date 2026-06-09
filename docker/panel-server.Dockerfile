# EMORELAY panel-server 多阶段镜像。
#
# - builder: 用 rust:1-slim-bookworm 编 release,带 protoc(common build.rs 用 tonic-build)。
# - runtime: 用 debian:bookworm-slim 跑二进制 + migrations,以非 root 用户启动。
#
# 构建上下文 = 项目根:
#   docker build -f docker/panel-server.Dockerfile -t emorelay/panel-server .

FROM rust:1-slim-bookworm AS builder

# pkg-config / libssl-dev: sqlx / jsonwebtoken 默认特性可能引 OpenSSL。
# ca-certificates: cargo 拉 crates.io / git 索引走 HTTPS。
# 不装 protobuf-compiler:common crate 的 build.rs 通过 `protoc-bin-vendored`
# 自带平台 protoc,系统 protoc 反而可能版本错乱。
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# musl 工具链 + 两个 linux target,用于编静态 agent 二进制。
# musl-tools 提供 x86_64-linux-musl-gcc 链接 amd64；gcc-aarch64-linux-gnu 链接 arm64。
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools gcc-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

ENV CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc

WORKDIR /build

# 全量拷源(.dockerignore 已剔除 target/、node_modules、文档、本地 db 等)。
# 不做依赖缓存层是因为 cargo 的 workspace 依赖图与 src 高度耦合,精细缓存增益有限。
# 构建慢可通过 BuildKit cache mount 优化(见 docs/deployment.md)。
COPY . .

RUN cargo build --release -p panel-server

# cross-compile node-agent 两个 linux musl target,产物给 /install.sh 端点 serve。
RUN cargo build --release -p node-agent --target x86_64-unknown-linux-musl
RUN cargo build --release -p node-agent --target aarch64-unknown-linux-musl


FROM debian:bookworm-slim AS runtime

# curl: 给 docker-compose healthcheck 用(debian-slim 默认不带)。
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/*

# 固定 UID 方便 host 用户与挂载卷对齐权限。
RUN useradd -r -u 1001 -s /usr/sbin/nologin emorelay

COPY --from=builder /build/target/release/panel-server /usr/local/bin/panel-server
COPY --from=builder /build/migrations /app/migrations

COPY --from=builder \
  /build/target/x86_64-unknown-linux-musl/release/node-agent \
  /var/lib/emorelay/agent-dist/node-agent-linux-amd64
COPY --from=builder \
  /build/target/aarch64-unknown-linux-musl/release/node-agent \
  /var/lib/emorelay/agent-dist/node-agent-linux-arm64

# /var/lib/emorelay 给 sqlite + agent-dist + 未来 TLS 材料;/app 仅为 migrations 落点。
RUN mkdir -p /var/lib/emorelay/agent-dist && chown -R emorelay:emorelay /var/lib/emorelay /app

RUN chmod 0755 /var/lib/emorelay/agent-dist/node-agent-linux-amd64 \
               /var/lib/emorelay/agent-dist/node-agent-linux-arm64 \
    && chown -R emorelay:emorelay /var/lib/emorelay/agent-dist

USER emorelay
WORKDIR /app

# 8080: REST API;50051: gRPC 控制面(Agent 入口)。
EXPOSE 8080 50051

ENTRYPOINT ["panel-server"]
