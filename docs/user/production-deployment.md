# Docker Compose 生产部署

> 状态：当前。

`docker-compose.prd.yaml` 在单台主机上运行 Gateway 与 PostgreSQL。Gateway
镜像内已嵌入 Console Web UI；生产环境不需要常驻 Node 进程。该方案是单节点基线，
不提供 PostgreSQL 高可用、PITR、跨主机 spool 复制或 TLS 终止。

## 1. 准备配置与密钥

从仓库根目录执行：

```bash
mkdir -p ./config
cp deploy/compose/config.example.toml ./config/config.prd.toml
cp deploy/compose/env.example ./config/compose.prd.env

openssl rand -hex 32 > ./config/postgres-password
openssl genpkey -algorithm Ed25519 \
  -out ./config/console-jwt-private.pem
openssl pkey \
  -in ./config/console-jwt-private.pem \
  -pubout \
  -out ./config/console-jwt-public.pem

chmod 600 \
  ./config/config.prd.toml \
  ./config/compose.prd.env \
  ./config/postgres-password \
  ./config/console-jwt-private.pem
chmod 644 ./config/console-jwt-public.pem
```

检查 `config.prd.toml`：

- `[server]` 和 `[console]` 必须在容器内监听 `0.0.0.0`。
- `[database].url` 默认使用 Compose 服务名 `postgres`。
- 数据库密码与 JWT 密钥路径指向 `/run/ai-gateway/secrets/*`。
- request-log spool 位于持久卷
  `/var/lib/ai-gateway/request-log-spool`。
- 如浏览器不是从 Console listener 同源访问，配置准确的
  `allowed_origins`；不要使用 `*`。

应用不会读取 `.env`。`compose.prd.env` 只由 Docker Compose 展开，
Gateway 仍只读取 TOML。

## 2. 选择拉取或本地构建

### 拉取已发布镜像

`AI_GATEWAY_VERSION` 应固定到不可变版本，不要在生产中依赖 `latest`：

```bash
docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  pull gateway
```

私有 GHCR Package 需要先执行 `docker login ghcr.io`。tag 发布工作流会同时
发布 GHCR 镜像与 GitHub Release 二进制包；若当前主机不能访问 GHCR，则使用
下方本地构建。

### 从当前 checkout 构建

```bash
docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  build gateway
```

Dockerfile 使用 Rust 1.85 构建 release 二进制，并先构建、再嵌入 Console
Web UI。运行镜像只保留二进制及必要的 CA、健康检查和权限切换工具。

## 3. 启动与验证

```bash
docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  up -d --no-build

docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  ps

curl -i http://127.0.0.1:3000/health
```

PostgreSQL 不映射到宿主机端口，只能通过 Compose 网络或
`docker compose exec postgres ...` 访问。数据面和 Console 端口默认仅绑定
宿主机 `127.0.0.1`，应由反向代理按需暴露。

Console refresh cookie 带 `Secure` 属性。浏览器生产访问必须经过 HTTPS
反向代理；Gateway 本身不终止 TLS。

## 4. 创建首个管理员

密码仅通过标准输入传入：

```bash
docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  run --rm -T gateway \
  bootstrap-admin \
  --email admin@example.com \
  --display-name "Initial Admin" \
  --password-stdin \
  --config /run/ai-gateway/config.toml \
  < /secure/path/admin-password.txt
```

## 5. 升级

1. 备份 PostgreSQL，并确认本地 spool 与 `request_log_ingest` 已排空；包含 journal payload 破坏性变更的升级不能保留旧二进制写入的积压记录。
2. 将 `config/compose.prd.env` 中的 `AI_GATEWAY_VERSION` 改为目标版本。
3. 拉取镜像或在对应 tag 的 checkout 上重新构建。
4. 执行 `up -d --no-build`；Gateway 启动时自动运行 migration。
5. 检查 `/health`、容器日志、spool/ingress/settlement backlog 和 Console。

Migration `0017_remove_legacy_compatibility.sql` 会永久删除
`api_keys.tokens_per_minute` 与 `channels.health_check` 的值；升级前备份必须可用。

不要删除 `postgres-data` 或 `gateway-spool` volume 来完成升级。回滚应用版本前，
先确认新 migration 是否向后兼容；数据库回滚必须依赖经过演练的备份恢复方案。

## 安全与耐久边界

- 配置与密钥不会烤入镜像。容器 entrypoint 以 root 读取只读 mount，复制到私有
  tmpfs 后降权为 UID/GID `10001` 运行 Gateway。
- 根文件系统只读；request-log spool 使用独立持久卷。
- Compose 的 PostgreSQL 参数与 `docker-compose.yml` 使用相同的单节点基线。
- 生产必须另行设计 TLS、备份、PITR、监控、告警和高可用。
