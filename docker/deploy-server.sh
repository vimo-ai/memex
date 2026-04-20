#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
IMAGE_NAME="memex-server"
CONTAINER_NAME="memex-server"
TAG="${1:-latest}"

NAS_HOST="${NAS_HOST:-10.0.0.1}"
NAS_USER="${NAS_USER:-aguai}"
NAS_DIR="/volume2/docker/${CONTAINER_NAME}"

MEMEX_API_KEYS="${MEMEX_API_KEYS:?请设置 MEMEX_API_KEYS 环境变量}"
TLS_CERT="${TLS_CERT:-}"
TLS_KEY="${TLS_KEY:-}"

echo "=== Memex Server 部署 ==="
echo "目标: ${NAS_USER}@${NAS_HOST}"
echo "镜像: ${IMAGE_NAME}:${TAG}"

# --- 构建镜像 ---
if ! docker image inspect "${IMAGE_NAME}:${TAG}" &>/dev/null; then
    BINARY="${SCRIPT_DIR}/build/amd64/memex-server"

    if [ ! -f "${BINARY}" ]; then
        echo "请将 memex-server-linux-x64 放到 ${BINARY}"
        echo "下载: gh run download --repo vimo-ai/memex --name memex-server-linux-x64 --dir ${SCRIPT_DIR}/build/amd64"
        exit 1
    fi

    CONTEXT=$(mktemp -d)
    cp "${BINARY}" "${CONTEXT}/memex-server"
    cp "${SCRIPT_DIR}/Dockerfile.server" "${CONTEXT}/Dockerfile"
    chmod +x "${CONTEXT}/memex-server"

    docker build --platform linux/amd64 -t "${IMAGE_NAME}:${TAG}" "${CONTEXT}"
    rm -rf "${CONTEXT}"
fi

# --- 传输镜像 ---
echo "传输镜像到 NAS..."
docker save "${IMAGE_NAME}:${TAG}" | ssh "${NAS_USER}@${NAS_HOST}" "/usr/local/bin/docker load"

# --- 写入 env file ---
echo "写入配置..."
ENV_CONTENT="RUST_LOG=memex=info
MEMEX_API_KEYS=${MEMEX_API_KEYS}"

if [ -n "${TLS_CERT}" ] && [ -n "${TLS_KEY}" ]; then
    ENV_CONTENT="${ENV_CONTENT}
TLS_CERT=/certs/$(basename "${TLS_CERT}")
TLS_KEY=/certs/$(basename "${TLS_KEY}")"

    ssh -T "${NAS_USER}@${NAS_HOST}" "mkdir -p ${NAS_DIR}/certs"
    scp -O "${TLS_CERT}" "${TLS_KEY}" "${NAS_USER}@${NAS_HOST}:${NAS_DIR}/certs/"
fi

ssh -T "${NAS_USER}@${NAS_HOST}" "mkdir -p ${NAS_DIR}/data/db && cat > ${NAS_DIR}/.env && chmod 600 ${NAS_DIR}/.env" <<< "${ENV_CONTENT}"

# --- 部署容器 ---
echo "部署容器..."

TLS_VOLUME=""
if [ -n "${TLS_CERT}" ]; then
    TLS_VOLUME="-v ${NAS_DIR}/certs:/certs:ro"
fi

ssh -T "${NAS_USER}@${NAS_HOST}" << EOF
set -e
/usr/local/bin/docker stop ${CONTAINER_NAME} 2>/dev/null || true
/usr/local/bin/docker rm -f ${CONTAINER_NAME} 2>/dev/null || true

/usr/local/bin/docker run -d \
    --name ${CONTAINER_NAME} \
    --restart unless-stopped \
    --user root \
    -p 10013:10013 \
    -v ${NAS_DIR}/data:/data \
    ${TLS_VOLUME} \
    --env-file ${NAS_DIR}/.env \
    ${IMAGE_NAME}:${TAG}

sleep 3
/usr/local/bin/docker ps --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}' | grep ${CONTAINER_NAME}
EOF

PROTO="http"
[ -n "${TLS_CERT}" ] && PROTO="https"

echo ""
echo "=== 部署完成 ==="
echo "Health: curl ${PROTO}://${NAS_HOST}:10013/api/sync/health"
echo "Push:   POST ${PROTO}://${NAS_HOST}:10013/api/sync/push"
