# EMORELAY web 前端镜像。
# builder: Vite + TSC 编静态产物 → nginx:alpine 提供 + /api/* 反代 panel-server。
#
# 构建上下文 = 项目根:
#   docker build -f docker/web.Dockerfile -t emorelay/web .

FROM node:20-alpine AS builder

WORKDIR /web

# 先拷 lockfile + manifest 走 npm ci 缓存层。
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund

# 源码增量层。
COPY web/. ./
RUN npm run build


FROM nginx:alpine AS runtime

# SPA 静态文件。
COPY --from=builder /web/dist /usr/share/nginx/html

# 自带 server 块:SPA fallback + /api 反代到 panel-server。
COPY docker/web-nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 80
