#!/usr/bin/env bash
# EMORELAY 主控一键部署脚本（Debian 12/13）
#
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/Remine1337/EMORELAY/master/deploy.sh | bash
# 或在已 clone 的仓库根目录:
#   bash deploy.sh
#
# 提供 docker compose / systemd 裸机两种安装方式,以及升级/状态/日志/备份/卸载菜单。
# 注意:本脚本全程不开 set -x,避免凭据(JWT secret / 管理员密码)落入终端回放或日志。
set -euo pipefail

REPO_URL="https://github.com/Remine1337/EMORELAY.git"
RAW_DEPLOY_URL="https://raw.githubusercontent.com/Remine1337/EMORELAY/master/deploy.sh"
GH_REPO="Remine1337/EMORELAY"
RELEASE_LATEST_URL="https://github.com/${GH_REPO}/releases/latest"
INSTALL_DIR="/opt/emorelay"
DATA_DIR="/var/lib/emorelay"
ENV_DIR="/etc/emorelay"
ENV_FILE="${ENV_DIR}/panel.env"
MODE_FILE="${ENV_DIR}/deploy-mode"
UNIT_NAME="emorelay-panel"
UNIT_FILE="/etc/systemd/system/${UNIT_NAME}.service"
PANEL_BIN="/usr/local/bin/emorelay-panel"
WEB_ROOT="/var/www/emorelay"
CADDYFILE="/etc/caddy/Caddyfile"
CADDY_BAK="/etc/caddy/Caddyfile.bak-emorelay"

# 测试逃生阀:容器冒烟环境无 systemd 时置 1,跳过所有 systemctl 调用。
SKIP_SYSTEMCTL="${EMORELAY_SKIP_SYSTEMCTL:-0}"

# GitHub 下载加速前缀(对标 flux ghfast.top)。中国大陆网络自动启用,显式设
# EMORELAY_GH_PROXY=（空)可禁用,或设为自定义镜像(末尾带 /)。gh_url 包裹 GitHub URL。
GH_PROXY="${EMORELAY_GH_PROXY-}"
GH_PROXY_DETECTED=0
# 自动探测:仅当用户未显式设置该 env 时,据 Cloudflare trace 的 loc 判断是否在 CN。
detect_cn_proxy() {
    [[ -n "${EMORELAY_GH_PROXY+x}" ]] && return  # 显式设置(含空)→ 尊重,不自动探测
    local loc
    loc="$(curl -fsS --max-time 3 https://www.cloudflare.com/cdn-cgi/trace 2>/dev/null | sed -n 's/^loc=//p' | tr -d '\r')"
    if [[ "$loc" == "CN" ]]; then
        GH_PROXY="https://ghfast.top/"
        GH_PROXY_DETECTED=1
    fi
}
# 用加速前缀包裹一个 github.com / raw.githubusercontent.com URL。GH_PROXY 为空则原样返回。
gh_url() {
    if [[ -n "$GH_PROXY" ]]; then
        echo "${GH_PROXY}$1"
    else
        echo "$1"
    fi
}

# ---------------------------------------------------------------------------
# 工具层
# ---------------------------------------------------------------------------
C_RED=$'\033[31m'; C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'; C_RESET=$'\033[0m'

info() { echo "${C_GREEN}[信息]${C_RESET} $*"; }
warn() { echo "${C_YELLOW}[警告]${C_RESET} $*"; }
err()  { echo "${C_RED}[错误]${C_RESET} $*" >&2; }
die()  { err "$@"; exit 1; }

run_systemctl() {
    if [[ "$SKIP_SYSTEMCTL" == "1" ]]; then
        warn "(EMORELAY_SKIP_SYSTEMCTL=1) 跳过: systemctl $*"
        return 0
    fi
    systemctl "$@"
}

gen_secret() { openssl rand -hex 32; }
gen_password() { openssl rand -base64 18 | tr -d '\n'; }

# 带默认值读取一行输入。curl|bash 场景下 stdin 已被 ensure_tty 重定向到 /dev/tty。
prompt() {
    local message="$1" default="${2:-}" reply
    if [[ -n "$default" ]]; then
        read -r -p "$message [默认: ${default}]: " reply
        echo "${reply:-$default}"
    else
        read -r -p "$message: " reply
        echo "$reply"
    fi
}

# 隐藏回显读取敏感输入(密码),换行补到 stderr 以免污染命令替换的捕获值。
prompt_secret() {
    local message="$1" reply
    read -r -s -p "$message: " reply
    printf '\n' >&2
    echo "$reply"
}

confirm() {
    local message="$1" reply
    read -r -p "$message [y/N]: " reply
    [[ "$reply" == "y" || "$reply" == "Y" ]]
}

# ---------------------------------------------------------------------------
# 环境检测
# ---------------------------------------------------------------------------
ensure_tty() {
    if [[ ! -t 0 ]]; then
        if [[ -e /dev/tty && -r /dev/tty ]]; then
            exec </dev/tty
        else
            err "检测不到交互终端,无法显示安装菜单。"
            err "请先下载脚本再执行:"
            err "  curl -fsSLO ${RAW_DEPLOY_URL} && bash deploy.sh"
            exit 1
        fi
    fi
}

check_root() {
    [[ "$(id -u)" -eq 0 ]] || die "请以 root 运行(sudo bash deploy.sh)。"
}

check_os() {
    [[ -f /etc/os-release ]] || die "缺少 /etc/os-release,无法识别系统。"
    # shellcheck source=/dev/null
    . /etc/os-release
    [[ "${ID:-}" == "debian" ]] || die "仅支持 Debian,检测到: ${ID:-未知} ${VERSION_ID:-}"
    DEBIAN_VERSION="${VERSION_ID%%.*}"
    case "$DEBIAN_VERSION" in
        12|13) info "系统: Debian ${VERSION_ID}" ;;
        *) die "仅支持 Debian 12/13,检测到 Debian ${VERSION_ID:-未知}" ;;
    esac
}

check_arch() {
    ARCH="$(dpkg --print-architecture)"
    case "$ARCH" in
        amd64) MUSL_TARGET="x86_64-unknown-linux-musl" ;;
        arm64) MUSL_TARGET="aarch64-unknown-linux-musl" ;;
        *) die "仅支持 amd64/arm64,检测到: $ARCH" ;;
    esac
    info "架构: $ARCH"
}

# 编译 Rust workspace 至少需要约 3GB 可用内存(含 swap),不足时给出缓解选项。
CARGO_JOBS_FLAG=()
check_memory() {
    local mem_kb swap_kb total_mb
    mem_kb=$(awk '/^MemTotal:/{print $2}' /proc/meminfo)
    swap_kb=$(awk '/^SwapTotal:/{print $2}' /proc/meminfo)
    total_mb=$(( (mem_kb + swap_kb) / 1024 ))
    if (( total_mb < 3072 )); then
        warn "内存+swap 共 ${total_mb}MB,低于 3GB,源码编译可能 OOM。"
        if confirm "是否自动创建 2GB swap 文件(/swapfile-emorelay)?"; then
            if [[ ! -f /swapfile-emorelay ]]; then
                fallocate -l 2G /swapfile-emorelay || dd if=/dev/zero of=/swapfile-emorelay bs=1M count=2048
                chmod 600 /swapfile-emorelay
                mkswap /swapfile-emorelay
            fi
            swapon /swapfile-emorelay 2>/dev/null || true
            grep -q '/swapfile-emorelay' /etc/fstab || echo '/swapfile-emorelay none swap sw 0 0' >> /etc/fstab
            info "swap 已启用。"
        else
            warn "将以单线程编译(CARGO_BUILD_JOBS=1)降低内存峰值,编译时间会变长。"
            CARGO_JOBS_FLAG=(-j 1)
        fi
    fi
}

# 安装前预检关键端口,被占用则直接报错指明进程,避免装到一半互踩。
check_ports() {
    local ports="$1" line busy=0 p
    for p in $ports; do
        line=$(ss -tlnp "sport = :$p" 2>/dev/null | tail -n +2)
        if [[ -n "$line" ]]; then
            err "端口 $p 已被占用: $line"
            busy=1
        fi
    done
    (( busy == 0 )) || die "请先释放上述端口再安装。"
}

# 读取部署模式标记并与实际状态双重校验,漂移时清理标记视为未安装。
# MODE_FILE 格式:第 1 行模式(docker/systemd),第 2 行安装目录(支持仓库内安装后异地重跑),
# 第 3 行安装通道(source/release,缺省 source,向后兼容旧标记)。
detect_mode() {
    DEPLOY_MODE=""
    INSTALL_CHANNEL="source"
    [[ -f "$MODE_FILE" ]] || return 0
    DEPLOY_MODE="$(head -n1 "$MODE_FILE")"
    local recorded_dir recorded_channel
    recorded_dir="$(sed -n '2p' "$MODE_FILE")"
    recorded_channel="$(sed -n '3p' "$MODE_FILE")"
    if [[ -n "$recorded_dir" && -d "$recorded_dir" ]]; then
        INSTALL_DIR="$recorded_dir"
    fi
    if [[ "$recorded_channel" == "release" ]]; then
        INSTALL_CHANNEL="release"
    fi
    case "$DEPLOY_MODE" in
        systemd)
            if [[ ! -f "$UNIT_FILE" ]]; then
                warn "状态不一致: 标记为 systemd 安装但 unit 文件不存在,视为未安装。"
                rm -f "$MODE_FILE"; DEPLOY_MODE=""
            fi
            ;;
        docker)
            if [[ ! -f "${INSTALL_DIR}/docker-compose.yml" ]] || ! command -v docker >/dev/null 2>&1; then
                warn "状态不一致: 标记为 docker 安装但 compose 文件或 docker 不存在,视为未安装。"
                rm -f "$MODE_FILE"; DEPLOY_MODE=""
            fi
            ;;
        *)
            warn "未知部署模式标记 '$DEPLOY_MODE',已清理。"
            rm -f "$MODE_FILE"; DEPLOY_MODE=""
            ;;
    esac
}

# ---------------------------------------------------------------------------
# 仓库获取
# ---------------------------------------------------------------------------
ensure_base_pkgs() {
    info "安装基础工具(git curl ca-certificates openssl)..."
    apt-get update -qq
    apt-get install -y -qq git curl ca-certificates openssl >/dev/null
}

ensure_repo() {
    # 已在仓库内运行(开发者本地 clone 场景)则原地使用。
    if git rev-parse --show-toplevel >/dev/null 2>&1; then
        local top
        top="$(git rev-parse --show-toplevel)"
        if [[ -f "${top}/Cargo.toml" && -d "${top}/crates/panel-server" ]]; then
            INSTALL_DIR="$top"
            info "在已有仓库内运行: $INSTALL_DIR"
            return 0
        fi
    fi
    if [[ -d "${INSTALL_DIR}/.git" ]]; then
        info "源码已存在: $INSTALL_DIR"
    else
        info "clone 仓库到 $INSTALL_DIR ..."
        git clone --depth 1 "$(gh_url "$REPO_URL")" "$INSTALL_DIR"
    fi
}

# ---------------------------------------------------------------------------
# 配置收集
# ---------------------------------------------------------------------------
detect_public_ip() {
    ip route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src"){print $(i+1); exit}}'
}

# 参数: 部署模式(docker/systemd)。docker 模式不提供 HTTPS 选项(web 容器只有
# HTTP :80,自动 HTTPS 需手动配 host Caddy,参考 docker/Caddyfile.example)。
collect_config() {
    local mode="$1"
    echo
    info "===== 配置收集 ====="
    PANEL_HOST="$(prompt '面板访问域名(留空则用本机 IP 直连)' '')"
    USE_HTTPS=0
    if [[ -n "$PANEL_HOST" ]]; then
        if [[ "$mode" == "docker" ]]; then
            warn "docker 模式默认 HTTP(web 容器占 host:80)。要上 HTTPS 请装好后参考 docker/Caddyfile.example。"
        elif confirm "是否启用 Caddy 自动 HTTPS(需域名 A 记录已指向本机)?"; then
            USE_HTTPS=1
        fi
    else
        local detected
        detected="$(detect_public_ip || true)"
        PANEL_HOST="$(prompt '本机公网 IP' "$detected")"
        [[ -n "$PANEL_HOST" ]] || die "必须提供域名或 IP。"
    fi

    if (( USE_HTTPS )); then
        BASE_URL="https://${PANEL_HOST}"
    else
        BASE_URL="http://${PANEL_HOST}"
    fi

    # Agent 接入主机名与 Web 域名分开问:Web 域名走 CDN/Cloudflare 橙云时,
    # CDN 不转发 50051 且会终结 TLS,Agent 必须用直连源站的域名/IP。
    echo
    info "Agent 通过 gRPC(:50051)直连本机,不经过 CDN。"
    info "若上面的面板域名挂了 Cloudflare 橙云等代理,这里必须填能直连本机的域名或 IP(如灰云子域)。"
    GRPC_HOST="$(prompt 'Agent 接入主机名/IP(gRPC)' "$PANEL_HOST")"
    [[ -n "$GRPC_HOST" ]] || die "Agent 接入主机名不能为空。"

    echo
    warn "PANEL_PUBLIC_HOST=${GRPC_HOST} 会写入 gRPC server 证书 SAN,且【首次启动后固化】。"
    warn "之后更改需删除 ${DATA_DIR}/tls 重启,并给所有已接入 Agent 重装凭据。"
    confirm "确认使用 ${GRPC_HOST} 作为 Agent 接入主机名/IP?" || die "已取消,请重新运行并填写正确地址。"

    ADMIN_PASSWORD="$(prompt_secret '管理员密码(留空自动生成)')"
    ADMIN_PASSWORD_GENERATED=0
    if [[ -z "$ADMIN_PASSWORD" ]]; then
        ADMIN_PASSWORD="$(gen_password)"
        ADMIN_PASSWORD_GENERATED=1
    fi
    JWT_SECRET="$(gen_secret)"
    # 用户选择保留既有 env 文件时置 1,summary 据此提示沿用旧凭据而非打印未生效的新密码。
    ENV_KEPT=0
}

print_summary() {
    local mode="$1"
    echo
    echo "=============================================================="
    info "EMORELAY 安装完成(${mode} 模式)"
    echo "   控制台地址 : ${BASE_URL}"
    echo "   管理员账号 : admin"
    if (( ENV_KEPT )); then
        echo "   管理员凭据 : 沿用既有配置(本次未更换)"
    elif (( ADMIN_PASSWORD_GENERATED )); then
        echo "   管理员密码 : ${ADMIN_PASSWORD}   <-- 自动生成,仅本次显示,请立即保存"
    else
        echo "   管理员密码 : (你刚才输入的密码)"
    fi
    echo "   Agent 接入 : https://${GRPC_HOST}:50051 (mTLS 已默认强制)"
    echo
    warn "请登录后到「系统设置」页将 Agent 控制端点设为 https://${GRPC_HOST}:50051"
    if [[ "$mode" == "systemd" ]]; then
        echo "   常用命令   : systemctl status ${UNIT_NAME} / journalctl -u ${UNIT_NAME} -f"
    else
        echo "   常用命令   : docker compose -f ${INSTALL_DIR}/docker-compose.yml ps / logs -f panel-server"
    fi
    echo "   再次运行本脚本可进入 升级/状态/日志/备份/卸载 菜单。"
    echo "=============================================================="
    firewall_hint
}

firewall_hint() {
    local ports="80 50051"
    (( USE_HTTPS )) && ports="80 443 50051"
    if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q 'Status: active'; then
        warn "检测到 ufw 已启用,请放行端口:"
        local p; for p in $ports; do echo "   ufw allow ${p}/tcp"; done
    elif command -v nft >/dev/null 2>&1 && nft list ruleset 2>/dev/null | grep -q 'hook input'; then
        warn "检测到 nftables 有 input 规则,请确认已放行 TCP 端口: ${ports}"
    fi
}

# ---------------------------------------------------------------------------
# release 快速安装模式(预编译二进制,免 Rust/Node 工具链与 3GB 内存门槛)
# ---------------------------------------------------------------------------
# 跟随 /releases/latest 重定向解析最新 tag,无 release 时返回非零。
resolve_release_tag() {
    local url
    # tag 解析走直连 GitHub(重定向跟随,加速镜像未必透传 302);失败时调用方回落源码模式。
    url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$RELEASE_LATEST_URL" 2>/dev/null)" || return 1
    [[ "$url" == */tag/* ]] || return 1
    echo "${url##*/}"
}

# 下载 release 资产到 RELEASE_TMP 并做 SHA256 校验。资产名与 CI 产物是契约
# (见 .github/workflows/release.yml),agent 双架构一次拉齐,免交叉编译。
# 调用前若已置 RELEASE_TAG(升级路径先比版本)则不再重复解析。
RELEASE_TMP=""
fetch_release_assets() {
    if [[ -z "${RELEASE_TAG:-}" ]]; then
        RELEASE_TAG="$(resolve_release_tag)" \
            || die "无法解析最新 release(仓库可能尚未发版或网络受限),请改用源码编译安装。"
    fi
    info "最新 release: ${RELEASE_TAG}"
    RELEASE_TMP="$(mktemp -d)"
    # 二进制/前端是下载大头,走加速镜像(CN 自动启用);SHA256SUMS 仍校验,镜像被
    # 投毒会校验失败而中止,安全性不降。
    local base
    base="$(gh_url "https://github.com/${GH_REPO}/releases/download/${RELEASE_TAG}")"
    local f
    for f in "panel-server-linux-${ARCH}" node-agent-linux-amd64 node-agent-linux-arm64 web-dist.tar.gz SHA256SUMS; do
        info "下载 ${f} ..."
        curl -fSL --progress-bar -o "${RELEASE_TMP}/${f}" "${base}/${f}" \
            || die "下载 ${f} 失败,请检查网络后重试。"
    done
    # --ignore-missing:SHA256SUMS 含另一架构 panel 二进制,本机不下载。
    (cd "$RELEASE_TMP" && sha256sum -c --ignore-missing --quiet SHA256SUMS) \
        || die "SHA256 校验失败,下载内容可能损坏或被篡改,已中止。"
    info "SHA256 校验通过。"
}

deploy_release_artifacts() {
    install -m 0755 "${RELEASE_TMP}/panel-server-linux-${ARCH}" "$PANEL_BIN"
    install -d -m 0750 "${DATA_DIR}" "${DATA_DIR}/agent-dist"
    install -m 0755 "${RELEASE_TMP}/node-agent-linux-amd64" \
        "${DATA_DIR}/agent-dist/node-agent-linux-amd64"
    install -m 0755 "${RELEASE_TMP}/node-agent-linux-arm64" \
        "${DATA_DIR}/agent-dist/node-agent-linux-arm64"
    chown -R emorelay:emorelay "$DATA_DIR"
    # 前端原子替换,与 deploy_artifacts 同策略。
    rm -rf "${WEB_ROOT}.new"
    mkdir -p "${WEB_ROOT}.new"
    tar xzf "${RELEASE_TMP}/web-dist.tar.gz" -C "${WEB_ROOT}.new"
    if [[ -d "$WEB_ROOT" ]]; then
        rm -rf "${WEB_ROOT}.old"
        mv "$WEB_ROOT" "${WEB_ROOT}.old"
    fi
    mv "${WEB_ROOT}.new" "$WEB_ROOT"
    rm -rf "${WEB_ROOT}.old"
    chown -R caddy:caddy "$WEB_ROOT" 2>/dev/null || true
    rm -rf "$RELEASE_TMP"
}

install_release_mode() {
    check_ports "80 8080 50051"
    ensure_base_pkgs
    collect_config systemd
    if (( USE_HTTPS )); then
        check_ports "443"
    fi
    install_caddy
    ensure_user
    fetch_release_assets
    deploy_release_artifacts
    write_panel_env
    write_unit
    write_caddyfile
    printf '%s\n%s\n%s\n' "systemd" "$INSTALL_DIR" "release" > "$MODE_FILE"
    echo "$RELEASE_TAG" > "${ENV_DIR}/release-tag"
    enable_services
    wait_healthy || warn "可运行本脚本选「查看日志」排查。"
    print_summary systemd
}

# ---------------------------------------------------------------------------
# docker 模式
# ---------------------------------------------------------------------------
install_docker() {
    if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
        info "Docker 与 compose plugin 已安装,跳过。"
    else
        info "安装 Docker CE(官方 get.docker.com 脚本)..."
        curl -fsSL https://get.docker.com | sh
    fi
    run_systemctl enable --now docker
}

write_docker_env() {
    local env_path="${INSTALL_DIR}/.env"
    if [[ -f "$env_path" ]]; then
        if ! confirm "检测到已有 ${env_path},是否覆盖重新生成?"; then
            info "保留现有 .env。"
            ENV_KEPT=1
            return 0
        fi
    fi
    umask 077
    cat > "$env_path" <<EOF
# 由 deploy.sh 生成。含密钥,权限保持 0600。
PANEL_JWT_SECRET=${JWT_SECRET}
PANEL_BOOTSTRAP_ADMIN_PASSWORD=${ADMIN_PASSWORD}
PANEL_CORS_ORIGIN=${BASE_URL}
PANEL_PUBLIC_BASE_URL=${BASE_URL}
PANEL_PUBLIC_HOST=${GRPC_HOST}
RUST_LOG=info,sqlx=warn
EOF
    umask 022
    info "已生成 ${env_path}(0600)。"
}

wait_healthy() {
    info "等待 panel-server 健康检查通过..."
    for _ in $(seq 1 30); do
        if curl -fsS http://127.0.0.1:8080/api/health >/dev/null 2>&1; then
            info "panel-server 已就绪。"
            return 0
        fi
        sleep 2
    done
    err "等待 60 秒后 /api/health 仍未通过,请检查日志。"
    return 1
}

install_docker_mode() {
    check_ports "80 8080 50051"
    ensure_base_pkgs
    ensure_repo
    collect_config docker
    install_docker
    write_docker_env
    info "构建并启动容器(首次构建需 5-15 分钟)..."
    docker compose -f "${INSTALL_DIR}/docker-compose.yml" --project-directory "$INSTALL_DIR" up -d --build
    install -d -m 0750 "$ENV_DIR"
    printf '%s\n%s\n' "docker" "$INSTALL_DIR" > "$MODE_FILE"
    wait_healthy || warn "可运行本脚本选「查看日志」排查。"
    print_summary docker
}

# ---------------------------------------------------------------------------
# systemd 裸机模式
# ---------------------------------------------------------------------------
install_build_deps() {
    info "安装编译依赖(build-essential pkg-config libssl-dev musl-tools)..."
    apt-get install -y -qq build-essential pkg-config libssl-dev musl-tools >/dev/null
}

install_rust() {
    if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
        info "Rust 已安装,跳过。"
    else
        info "安装 Rust(rustup, stable, minimal)..."
        curl -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
    fi
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
}

install_node() {
    if command -v node >/dev/null 2>&1 && [[ "$(node -e 'console.log(process.versions.node.split(".")[0])')" -ge 20 ]]; then
        info "Node $(node --version) 已满足要求,跳过。"
        return 0
    fi
    info "安装 Node.js 22(NodeSource)..."
    if curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && apt-get install -y -qq nodejs >/dev/null; then
        info "Node $(node --version) 安装完成。"
    elif [[ "$DEBIAN_VERSION" == "13" ]]; then
        warn "NodeSource 安装失败,回退到 Debian 13 自带 nodejs(20.x)。"
        apt-get install -y -qq nodejs npm >/dev/null
    else
        die "Node.js 安装失败(Debian 12 自带版本过老,且 NodeSource 不可用)。"
    fi
}

install_caddy() {
    if command -v caddy >/dev/null 2>&1; then
        info "Caddy 已安装,跳过。"
        return 0
    fi
    info "安装 Caddy(官方 apt 源)..."
    apt-get install -y -qq debian-keyring debian-archive-keyring apt-transport-https >/dev/null
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' \
        | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg --yes
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
        > /etc/apt/sources.list.d/caddy-stable.list
    apt-get update -qq
    apt-get install -y -qq caddy >/dev/null
}

ensure_user() {
    if ! id -u emorelay >/dev/null 2>&1; then
        useradd --system --no-create-home --shell /usr/sbin/nologin emorelay
        info "已创建系统用户 emorelay。"
    fi
}

build_all() {
    info "编译 panel-server(release,首次约 5-15 分钟)..."
    (cd "$INSTALL_DIR" && cargo build --release -p panel-server "${CARGO_JOBS_FLAG[@]}")
    info "编译 node-agent(${MUSL_TARGET} 静态二进制,供节点一键安装下载)..."
    rustup target add "$MUSL_TARGET"
    (cd "$INSTALL_DIR" && cargo build --release -p node-agent --target "$MUSL_TARGET" "${CARGO_JOBS_FLAG[@]}")
    info "构建前端(npm ci && npm run build)..."
    (cd "${INSTALL_DIR}/web" && npm ci --no-audit --no-fund && npm run build)
}

deploy_artifacts() {
    install -m 0755 "${INSTALL_DIR}/target/release/panel-server" "$PANEL_BIN"
    install -d -m 0750 "${DATA_DIR}" "${DATA_DIR}/agent-dist"
    install -m 0755 "${INSTALL_DIR}/target/${MUSL_TARGET}/release/node-agent" \
        "${DATA_DIR}/agent-dist/node-agent-linux-${ARCH}"
    chown -R emorelay:emorelay "$DATA_DIR"
    # 前端原子替换:先复制到 .new 再 mv,避免升级窗口期 Caddy 服务到半成品。
    rm -rf "${WEB_ROOT}.new"
    mkdir -p "$(dirname "$WEB_ROOT")"
    cp -r "${INSTALL_DIR}/web/dist" "${WEB_ROOT}.new"
    if [[ -d "$WEB_ROOT" ]]; then
        rm -rf "${WEB_ROOT}.old"
        mv "$WEB_ROOT" "${WEB_ROOT}.old"
    fi
    mv "${WEB_ROOT}.new" "$WEB_ROOT"
    rm -rf "${WEB_ROOT}.old"
    chown -R caddy:caddy "$WEB_ROOT" 2>/dev/null || true
}

write_panel_env() {
    if [[ -f "$ENV_FILE" ]]; then
        if ! confirm "检测到已有 ${ENV_FILE},是否覆盖重新生成?(覆盖会换 JWT secret,所有已登录会话失效)"; then
            info "保留现有 panel.env。"
            ENV_KEPT=1
            return 0
        fi
    fi
    install -d -m 0750 "$ENV_DIR"
    umask 077
    cat > "$ENV_FILE" <<EOF
# 由 deploy.sh 生成。systemd 以 root 读取本文件,权限保持 0600。
RUST_LOG=info,sqlx=warn
# REST 只绑本机回环,由 Caddy 反代;gRPC 直接对外(mTLS 自保护)。
PANEL_BIND_ADDR=127.0.0.1:8080
PANEL_GRPC_BIND_ADDR=0.0.0.0:50051
PANEL_DATABASE_URL=sqlite://${DATA_DIR}/emorelay.db
PANEL_DATA_DIR=${DATA_DIR}
PANEL_JWT_SECRET=${JWT_SECRET}
PANEL_JWT_EXPIRY_HOURS=24
PANEL_CORS_ORIGIN=${BASE_URL}
PANEL_BOOTSTRAP_ADMIN_USERNAME=admin
# 首次成功登录后可清空本行(已建库即忽略),减少一处明文。
PANEL_BOOTSTRAP_ADMIN_PASSWORD=${ADMIN_PASSWORD}
PANEL_PUBLIC_BASE_URL=${BASE_URL}
PANEL_PUBLIC_HOST=${GRPC_HOST}
PANEL_DEV_DISABLE_MTLS=0
EOF
    umask 022
    info "已生成 ${ENV_FILE}(0600)。"
}

write_unit() {
    cat > "$UNIT_FILE" <<EOF
[Unit]
Description=EMORELAY Panel Server (control plane: REST + gRPC)
Documentation=https://github.com/Remine1337/EMORELAY
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=emorelay
Group=emorelay
WorkingDirectory=${DATA_DIR}
ExecStart=${PANEL_BIN}
EnvironmentFile=${ENV_FILE}

Restart=on-failure
RestartSec=5s

# 安全收紧(与 scripts/emorelay-agent.service 同风格)
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
# SQLite 库 + WAL/SHM、内置 CA tls/、agent-dist 都在数据目录下,必须显式放行写。
ReadWritePaths=${DATA_DIR}
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
RestrictNamespaces=true
LockPersonality=true
RestrictRealtime=true
RestrictSUIDSGID=true
MemoryDenyWriteExecute=true
SystemCallArchitectures=native
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX

[Install]
WantedBy=multi-user.target
EOF
    info "已写入 ${UNIT_FILE}。"
}

write_caddyfile() {
    local site
    if (( USE_HTTPS )); then
        site="$PANEL_HOST"
    else
        site=":80"
    fi
    if [[ -f "$CADDYFILE" && ! -f "$CADDY_BAK" ]]; then
        cp "$CADDYFILE" "$CADDY_BAK"
        info "原 Caddyfile 已备份为 ${CADDY_BAK}。"
    fi
    cat > "$CADDYFILE" <<EOF
# EMORELAY 主控站点 — 由 deploy.sh 生成
${site} {
    encode gzip zstd

    # REST API(panel-server 只绑 127.0.0.1:8080)。
    # 必须用 handle 而非 handle_path:panel 路由自带 /api 前缀,剥掉会 404。
    handle /api/* {
        reverse_proxy 127.0.0.1:8080
    }
    # Agent 一键安装脚本与二进制分发,不在 /api 前缀下。
    handle /install.sh {
        reverse_proxy 127.0.0.1:8080
    }
    handle /dist/* {
        reverse_proxy 127.0.0.1:8080
    }

    # SPA 前端
    handle {
        root * ${WEB_ROOT}
        try_files {path} /index.html
        file_server
    }

    log {
        output file /var/log/caddy/emorelay.log
    }
}
EOF
    info "已写入 ${CADDYFILE}(站点: ${site})。"
}

enable_services() {
    run_systemctl daemon-reload
    run_systemctl enable --now "$UNIT_NAME"
    run_systemctl enable caddy
    run_systemctl restart caddy
}

install_systemd_mode() {
    check_ports "80 8080 50051"
    check_memory
    ensure_base_pkgs
    ensure_repo
    collect_config systemd
    if (( USE_HTTPS )); then
        check_ports "443"
    fi
    install_build_deps
    install_rust
    install_node
    install_caddy
    ensure_user
    build_all
    deploy_artifacts
    write_panel_env
    write_unit
    write_caddyfile
    printf '%s\n%s\n' "systemd" "$INSTALL_DIR" > "$MODE_FILE"
    enable_services
    wait_healthy || warn "可运行本脚本选「查看日志」排查。"
    print_summary systemd
    echo
    info "如有另一架构(amd64/arm64)的节点,再次运行本脚本选「补编另一架构 agent」。"
}

# 可选:交叉编译另一架构的 node-agent 放入 agent-dist。
build_agent_cross() {
    local other_arch other_target linker_pkg
    local -a cross_env
    if [[ "$ARCH" == "amd64" ]]; then
        other_arch="arm64"; other_target="aarch64-unknown-linux-musl"
        linker_pkg="gcc-aarch64-linux-gnu"
        # ring 等含 C 代码的依赖还需要目标 CC,只给 LINKER 不够。
        cross_env=(
            CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
            CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc
        )
    else
        other_arch="amd64"; other_target="x86_64-unknown-linux-musl"
        linker_pkg="gcc-x86-64-linux-gnu"
        cross_env=(
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-gnu-gcc
            CC_x86_64_unknown_linux_musl=x86_64-linux-gnu-gcc
        )
    fi
    info "交叉编译 ${other_target}(需额外下载约 200MB 工具链)..."
    apt-get install -y -qq "$linker_pkg" >/dev/null
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
    rustup target add "$other_target"
    (cd "$INSTALL_DIR" && env "${cross_env[@]}" cargo build --release -p node-agent --target "$other_target")
    install -m 0755 "${INSTALL_DIR}/target/${other_target}/release/node-agent" \
        "${DATA_DIR}/agent-dist/node-agent-linux-${other_arch}"
    chown emorelay:emorelay "${DATA_DIR}/agent-dist/node-agent-linux-${other_arch}"
    info "已放入 ${DATA_DIR}/agent-dist/node-agent-linux-${other_arch}。"
}

# ---------------------------------------------------------------------------
# 运维动作
# ---------------------------------------------------------------------------
action_backup() {
    local stamp dest
    stamp="$(date +%Y%m%d-%H%M%S)"
    dest="/root/emorelay-backup-${stamp}.tar.gz"
    info "备份会短暂停止 panel-server,保证 SQLite(WAL)一致性。"
    if [[ "$DEPLOY_MODE" == "systemd" ]]; then
        run_systemctl stop "$UNIT_NAME"
        tar czf "$dest" -C / "${DATA_DIR#/}" "${ENV_DIR#/}"
        run_systemctl start "$UNIT_NAME"
    else
        (cd "$INSTALL_DIR" && docker compose stop panel-server)
        local tmp
        tmp="$(mktemp -d)"
        (cd "$INSTALL_DIR" && docker compose cp panel-server:/var/lib/emorelay "${tmp}/emorelay-data")
        # .env 含 JWT secret 等凭据,缺了它恢复链不完整。
        if [[ -f "${INSTALL_DIR}/.env" ]]; then
            cp "${INSTALL_DIR}/.env" "${tmp}/emorelay.env"
        fi
        tar czf "$dest" -C "$tmp" .
        rm -rf "$tmp"
        (cd "$INSTALL_DIR" && docker compose start panel-server)
    fi
    info "备份完成: $dest"
    warn "备份包含 CA 私钥与配置密钥(JWT secret 等),请妥善保管。"
}

action_upgrade() {
    info "升级前先自动备份..."
    action_backup
    if [[ "$DEPLOY_MODE" == "systemd" && "$INSTALL_CHANNEL" == "release" ]]; then
        local current=""
        [[ -f "${ENV_DIR}/release-tag" ]] && current="$(cat "${ENV_DIR}/release-tag")"
        # 先比版本再下载,相同则免拉数十 MB 资产。
        RELEASE_TAG="$(resolve_release_tag)" \
            || die "无法解析最新 release,请检查网络后重试。"
        if [[ -n "$current" && "$RELEASE_TAG" == "$current" ]]; then
            info "已是最新版本(${current}),无需升级。"
            return 0
        fi
        fetch_release_assets
        info "升级 ${current:-未知版本} -> ${RELEASE_TAG},停止服务并替换产物(停机窗口为秒级)..."
        run_systemctl stop "$UNIT_NAME"
        deploy_release_artifacts
        run_systemctl start "$UNIT_NAME"
        echo "$RELEASE_TAG" > "${ENV_DIR}/release-tag"
        wait_healthy || warn "升级后健康检查未通过,请查看日志。"
        info "升级完成。"
        return 0
    fi
    info "拉取最新代码(git pull --ff-only)..."
    if ! git -C "$INSTALL_DIR" pull --ff-only; then
        die "git pull 失败(可能有本地改动或分叉),请到 ${INSTALL_DIR} 手动处理后重试。"
    fi
    if [[ "$DEPLOY_MODE" == "systemd" ]]; then
        # shellcheck source=/dev/null
        . "$HOME/.cargo/env"
        check_memory
        build_all
        info "停止服务并替换产物(停机窗口为秒级)..."
        run_systemctl stop "$UNIT_NAME"
        deploy_artifacts
        run_systemctl start "$UNIT_NAME"
    else
        (cd "$INSTALL_DIR" && docker compose up -d --build)
    fi
    wait_healthy || warn "升级后健康检查未通过,请查看日志。"
    info "升级完成。"
}

action_status() {
    if [[ "$DEPLOY_MODE" == "systemd" ]]; then
        run_systemctl status "$UNIT_NAME" caddy --no-pager || true
    else
        (cd "$INSTALL_DIR" && docker compose ps)
    fi
    echo
    if curl -fsS http://127.0.0.1:8080/api/health >/dev/null 2>&1; then
        info "/api/health 正常。"
    else
        warn "/api/health 不可达。"
    fi
    echo
    info "关键端口监听:"
    ss -tlnp | grep -E ':(80|443|8080|50051)\s' || echo "   (无)"
}

action_logs() {
    info "实时日志(Ctrl-C 退出)..."
    if [[ "$DEPLOY_MODE" == "systemd" ]]; then
        journalctl -u "$UNIT_NAME" -n 100 -f
    else
        (cd "$INSTALL_DIR" && docker compose logs --tail 100 -f panel-server)
    fi
}

action_uninstall() {
    warn "即将卸载 EMORELAY(${DEPLOY_MODE} 模式)。"
    confirm "确认卸载?" || { info "已取消。"; return 0; }
    if [[ "$DEPLOY_MODE" == "systemd" ]]; then
        run_systemctl disable --now "$UNIT_NAME" 2>/dev/null || true
        rm -f "$UNIT_FILE"
        run_systemctl daemon-reload
        rm -f "$PANEL_BIN"
        rm -rf "$WEB_ROOT"
        if [[ -f "$CADDY_BAK" ]]; then
            mv "$CADDY_BAK" "$CADDYFILE"
            run_systemctl restart caddy 2>/dev/null || true
            info "已恢复原 Caddyfile。"
        else
            warn "未找到 Caddyfile 备份,${CADDYFILE} 仍是 EMORELAY 配置,请手动处理。"
        fi
        if confirm "删除数据目录 ${DATA_DIR}?(含 SQLite 数据库与内置 CA,删除后所有 Agent 失联,不可恢复)"; then
            rm -rf "$DATA_DIR"
        fi
        if confirm "删除配置目录 ${ENV_DIR}?(含 JWT secret)"; then
            rm -rf "$ENV_DIR"
        else
            rm -f "$MODE_FILE"
        fi
        if confirm "删除源码目录 ${INSTALL_DIR}?"; then
            rm -rf "$INSTALL_DIR"
        fi
        if confirm "删除系统用户 emorelay?"; then
            userdel emorelay 2>/dev/null || true
        fi
        info "已卸载。Rust/Node/Caddy 软件包未移除,如需清理请手动 apt remove。"
    else
        (cd "$INSTALL_DIR" && docker compose down)
        if confirm "删除数据卷(sqlite-data)?(含 SQLite 数据库与内置 CA,删除后所有 Agent 失联,不可恢复)"; then
            (cd "$INSTALL_DIR" && docker compose down -v)
        fi
        if confirm "删除镜像 emorelay/panel-server:dev 与 emorelay/web:dev?"; then
            docker rmi emorelay/panel-server:dev emorelay/web:dev 2>/dev/null || true
        fi
        if confirm "删除源码目录 ${INSTALL_DIR}?(含 .env 密钥文件)"; then
            rm -rf "$INSTALL_DIR"
        fi
        rm -f "$MODE_FILE"
        rmdir "$ENV_DIR" 2>/dev/null || true
        info "已卸载。Docker 本身未移除,如需清理请手动处理。"
    fi
}

# ---------------------------------------------------------------------------
# 菜单
# ---------------------------------------------------------------------------
menu_not_installed() {
    echo
    echo "========== EMORELAY 部署菜单(未安装) =========="
    echo "  1) 快速安装 — GitHub Release 预编译二进制(推荐,免编译)"
    echo "  2) 安装 — Docker Compose(本机构建镜像)"
    echo "  3) 安装 — systemd 裸机(源码编译,Caddy 反代)"
    echo "  0) 退出"
    local choice
    choice="$(prompt '请选择' '0')"
    case "$choice" in
        1) install_release_mode ;;
        2) install_docker_mode ;;
        3) install_systemd_mode ;;
        0) exit 0 ;;
        *) err "无效选项: $choice" ;;
    esac
}

menu_installed() {
    echo
    echo "========== EMORELAY 管理菜单(已安装: ${DEPLOY_MODE} 模式) =========="
    echo "  1) 升级(git pull + 重新构建 + 重启)"
    echo "  2) 查看状态"
    echo "  3) 查看日志"
    echo "  4) 备份数据"
    echo "  5) 卸载"
    # release 通道资产已含双架构 agent,且机器上没有 cargo,无补编场景。
    if [[ "$DEPLOY_MODE" == "systemd" && "$INSTALL_CHANNEL" != "release" ]]; then
        echo "  6) 补编另一架构 agent 二进制"
    fi
    echo "  0) 退出"
    local choice
    choice="$(prompt '请选择' '0')"
    case "$choice" in
        1) action_upgrade ;;
        2) action_status ;;
        3) action_logs ;;
        4) action_backup ;;
        5) action_uninstall ;;
        6)
            if [[ "$DEPLOY_MODE" == "systemd" && "$INSTALL_CHANNEL" != "release" ]]; then
                build_agent_cross
            else
                err "无效选项: $choice"
            fi
            ;;
        0) exit 0 ;;
        *) err "无效选项: $choice" ;;
    esac
}

main() {
    ensure_tty
    check_root
    check_os
    check_arch
    detect_cn_proxy
    [[ "$GH_PROXY_DETECTED" == "1" ]] && \
        info "检测到中国大陆网络,GitHub 下载启用加速镜像 ${GH_PROXY}(EMORELAY_GH_PROXY= 可禁用)"
    detect_mode
    # 已安装时源码目录以标记安装位置为准(curl|bash 二次运行时 cwd 不在仓库内)。
    # release 通道无源码目录,升级走 release 资产,不需要仓库。
    if [[ -n "$DEPLOY_MODE" && -d "${INSTALL_DIR}/.git" ]]; then
        :
    elif [[ -n "$DEPLOY_MODE" && "$INSTALL_CHANNEL" != "release" ]]; then
        ensure_repo
    fi
    if [[ -z "$DEPLOY_MODE" ]]; then
        menu_not_installed
    else
        menu_installed
    fi
}

# exit 必须与 main 同一行:curl|bash 时 bash 增量读 stdin,而 ensure_tty 已把 fd0
# 换成 /dev/tty;若 main 返回后 bash 继续"读脚本",读到的将是用户键盘输入并被当
# root 命令执行。同行 exit 在同一次读入中解析,杜绝该路径。
main "$@"; exit "$?"
