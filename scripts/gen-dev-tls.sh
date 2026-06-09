#!/bin/sh
# EMORELAY 开发用 gRPC TLS 自签证书生成器(F7)。
#
# 生成:
#   $DIR/ca.crt      —— 自签 CA,Agent 用 AGENT_GRPC_CA_CERT 信任它
#   $DIR/server.crt  —— panel-server gRPC server cert (CN=localhost, SAN=127.0.0.1)
#   $DIR/server.key  —— panel-server gRPC server key
#   $DIR/agent.crt   —— mTLS client cert (CN=emorelay-agent), 由同一 CA 签发
#   $DIR/agent.key   —— mTLS client key
#
# 用法:
#   sh scripts/gen-dev-tls.sh           # 默认输出到 ./tls
#   sh scripts/gen-dev-tls.sh /opt/etc  # 指定目录
#
# 生产环境不要用本脚本——用 Caddy/Let's Encrypt 之类拿真实证书,
# 或在公司 CA 体系下签发,把证书路径配给 panel-server/agent。

set -e

DIR="${1:-./tls}"
mkdir -p "$DIR"

echo "→ generating self-signed CA in $DIR"

# 1. CA
openssl req -x509 -newkey rsa:4096 -nodes \
    -keyout "$DIR/ca.key" \
    -out "$DIR/ca.crt" \
    -days 3650 \
    -subj "/CN=emorelay-dev-ca"

# 2. server key + CSR
openssl genrsa -out "$DIR/server.key" 4096
openssl req -new -key "$DIR/server.key" \
    -out "$DIR/server.csr" \
    -subj "/CN=localhost"

# 3. SAN 扩展(同时覆盖 localhost / 127.0.0.1,生产请加上你的域名)
cat > "$DIR/server.ext" <<EOF
subjectAltName = DNS:localhost, IP:127.0.0.1
EOF

# 4. CA 签发 server cert
openssl x509 -req \
    -in "$DIR/server.csr" \
    -CA "$DIR/ca.crt" \
    -CAkey "$DIR/ca.key" \
    -CAcreateserial \
    -out "$DIR/server.crt" \
    -days 365 \
    -sha256 \
    -extfile "$DIR/server.ext"

# 5. client key + CSR (mTLS 用) — CN 任意, server 端只校验 CA 签名链
openssl genrsa -out "$DIR/agent.key" 4096
openssl req -new -key "$DIR/agent.key" \
    -out "$DIR/agent.csr" \
    -subj "/CN=emorelay-agent"

# 6. CA 签发 client cert
# 必须显式带 extendedKeyUsage = clientAuth,否则 rustls (tonic 0.12 默认 TLS)
# 的 WebPkiClientVerifier 会在 mTLS 握手阶段以 BadCertificate 拒绝该 cert。
cat > "$DIR/agent.ext" <<EOF
extendedKeyUsage = clientAuth
EOF

openssl x509 -req \
    -in "$DIR/agent.csr" \
    -CA "$DIR/ca.crt" \
    -CAkey "$DIR/ca.key" \
    -CAcreateserial \
    -out "$DIR/agent.crt" \
    -days 365 \
    -sha256 \
    -extfile "$DIR/agent.ext"

# 清理中间文件
rm -f "$DIR/server.csr" "$DIR/server.ext" "$DIR/agent.csr" "$DIR/agent.ext" "$DIR/ca.srl"

cat <<EOF

Done. 配置参考:

  panel-server (.env) — 单向 TLS:
    PANEL_GRPC_TLS_CERT=$DIR/server.crt
    PANEL_GRPC_TLS_KEY=$DIR/server.key

  panel-server (.env) — 启用 mTLS (在上面基础上加这行):
    PANEL_GRPC_TLS_CLIENT_CA=$DIR/ca.crt

  node-agent (.env or systemd EnvironmentFile):
    AGENT_CONTROL_ENDPOINT=https://localhost:50051
    AGENT_GRPC_CA_CERT=$DIR/ca.crt
    # mTLS 模式补这两行:
    AGENT_GRPC_CLIENT_CERT=$DIR/agent.crt
    AGENT_GRPC_CLIENT_KEY=$DIR/agent.key

Windows 用户:可用 git bash / WSL,或:
    docker run --rm -v "%cd%:/work" -w /work alpine/openssl sh scripts/gen-dev-tls.sh
EOF
