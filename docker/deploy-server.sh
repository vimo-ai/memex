#!/bin/bash
set -e

IMAGE_NAME="memex-server"
CONTAINER_NAME="memex-server"
TAG="${1:-latest}"

NAS_HOST="${NAS_HOST:-10.0.0.1}"
NAS_USER="${NAS_USER:-aguai}"
DATA_DIR="/volume2/docker/${CONTAINER_NAME}/data"

MEMEX_API_KEYS="${MEMEX_API_KEYS:?请设置 MEMEX_API_KEYS 环境变量}"
ENV_FILE="/volume2/docker/${CONTAINER_NAME}/.env"

echo "=== Memex Server 部署 ==="
echo "目标: ${NAS_USER}@${NAS_HOST}"
echo "镜像: ${IMAGE_NAME}:${TAG}"

# 如果本地没有镜像，从 GitHub Release 下载 binary 构建
if ! docker image inspect "${IMAGE_NAME}:${TAG}" &>/dev/null; then
    echo "本地无镜像，从 binary 构建..."

    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    BUILD_DIR="${SCRIPT_DIR}/build/amd64"
    mkdir -p "${BUILD_DIR}"

    if [ ! -f "${BUILD_DIR}/memex-server" ]; then
        echo "请将 memex-server-linux-x64 放到 ${BUILD_DIR}/memex-server"
        echo "下载地址: https://github.com/vimo-ai/memex/actions (Artifacts)"
        exit 1
    fi

    chmod +x "${BUILD_DIR}/memex-server"
    docker build --platform linux/amd64 \
        -f "${SCRIPT_DIR}/Dockerfile.server" \
        -t "${IMAGE_NAME}:${TAG}" \
        "${SCRIPT_DIR}"
fi

echo "传输镜像到 NAS..."
docker save "${IMAGE_NAME}:${TAG}" | ssh "${NAS_USER}@${NAS_HOST}" "/usr/local/bin/docker load"

echo "写入 env file..."
ssh "${NAS_USER}@${NAS_HOST}" "mkdir -p ${DATA_DIR}/db && cat > ${ENV_FILE}" << ENVEOF
RUST_LOG=memex=info
MEMEX_API_KEYS=${MEMEX_API_KEYS}
ENVEOF
ssh "${NAS_USER}@${NAS_HOST}" "chmod 600 ${ENV_FILE}"

echo "部署容器..."
ssh "${NAS_USER}@${NAS_HOST}" << EOF
set -e

/usr/local/bin/docker stop ${CONTAINER_NAME} 2>/dev/null || true
/usr/local/bin/docker rm -f ${CONTAINER_NAME} 2>/dev/null || true

/usr/local/bin/docker run -d \
    --name ${CONTAINER_NAME} \
    --restart unless-stopped \
    -p 10013:10013 \
    -v ${DATA_DIR}:/data \
    --env-file ${ENV_FILE} \
    ${IMAGE_NAME}:${TAG}

echo "等待启动..."
sleep 2
/usr/local/bin/docker ps | grep ${CONTAINER_NAME}
EOF

echo ""
echo "=== 部署完成 ==="
echo "Health: curl http://${NAS_HOST}:10013/api/sync/health"
echo "Push:   POST http://${NAS_HOST}:10013/api/sync/push"
