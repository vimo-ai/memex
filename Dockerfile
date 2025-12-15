# ============================================
# Memex - Claude Code 会话历史管理系统
# 多阶段构建: 前端 + 后端
# ============================================

# Stage 1: 前端构建
FROM node:20-alpine AS web-builder

WORKDIR /app/web

# 安装 pnpm
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate

# 复制前端依赖文件
COPY web/package.json web/pnpm-lock.yaml ./

# 安装依赖
RUN pnpm install --frozen-lockfile

# 复制前端源码
COPY web/ ./

# 构建前端
RUN pnpm build


# Stage 2: 后端构建
FROM node:20-alpine AS backend-builder

WORKDIR /app

# 安装 pnpm
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate

# 安装 better-sqlite3 编译依赖
RUN apk add --no-cache python3 make g++

# 复制后端依赖文件
COPY package.json pnpm-lock.yaml ./

# 安装依赖
RUN pnpm install --frozen-lockfile

# 复制后端源码
COPY src/ ./src/
COPY tsconfig.json nest-cli.json ./

# 构建后端
RUN pnpm build


# Stage 3: 生产镜像
FROM node:20-alpine AS production

WORKDIR /app

# 安装运行时依赖
RUN apk add --no-cache tini

# 安装 pnpm
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate

# 安装 better-sqlite3 运行时需要的库
RUN apk add --no-cache python3 make g++

# 复制依赖文件并安装生产依赖
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile --prod

# 复制构建产物
COPY --from=backend-builder /app/dist ./dist
COPY --from=web-builder /app/web/dist ./web/dist

# 创建数据目录
RUN mkdir -p /data /claude-sessions

# 环境变量
ENV NODE_ENV=production
ENV PORT=3000
ENV DATABASE_PATH=/data/memex.db
ENV CLAUDE_PROJECTS_PATH=/claude-sessions

# 暴露端口
EXPOSE 3000

# 使用 tini 作为 init 进程
ENTRYPOINT ["/sbin/tini", "--"]

# 启动应用
CMD ["node", "dist/main.js"]
