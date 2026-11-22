# Docker Compose 生产部署

> 状态：当前。单主机 PostgreSQL stack；SQLite 使用[单实例文件后端指南](sqlite.md)。

使用 `docker-compose.prd.yaml` 在单台主机上运行 Gateway、PostgreSQL 和嵌入式 Console。部署方负责 HTTPS 入口、可靠存储、监控和备份；该配置不提供 HA、PITR 或跨主机 spool 复制。

## 部署前确认

- 使用支持 Compose 的 Docker 主机，确认数据目录/卷容量和持久性。
- 固定 Compose 项目名和目标镜像版本，避免更换工作目录时连到空卷。
- 数据面和 Console 默认只映射宿主 `127.0.0.1:3000/3001`；PostgreSQL 不映射宿主端口。
- 以下配置命令用于新部署；已有环境先走[升级与回滚](upgrades.md)，不要覆盖秘密或初始化卷。

## 1. 准备配置与密钥

从仓库根目录执行：

```bash
umask 077
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
- Images edit 临时 body 位于独立本地卷
  `/var/lib/ai-gateway/image-edit-spool`；该卷不需要备份，但必须有足够容量且不能改成 RAM-backed
  `/tmp`。
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

私有 GHCR Package 需要先执行 `docker login ghcr.io`；无法访问镜像仓库时可从匹配 tag 的源码本地构建。

### 从当前 checkout 构建

```bash
docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  build gateway
```

Dockerfile 会构建并嵌入 Console 界面，运行镜像不需要 Node。

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

curl --fail-with-body -i http://127.0.0.1:3000/health
```

预期 Gateway 与 PostgreSQL 均 healthy，`/health` 返回 `204`。这只确认进程可响应；还需建立实际路由并核对计量。Console 的 refresh Cookie 带 `Secure`，请用 HTTPS 反向代理接入浏览器。

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

管理员密码至少 12 字节。`bootstrap-admin` 只在没有 active admin 时成功；密码文件须由部署者保护，不放进版本库。然后按[路由指南](routing.md)建立凭证、接入、渠道、能力、模型规则及 Key 授权；空数据库启动后不能直接代理。

## 5. 记录存储与运行身份

| Compose 卷 | 容器路径 | 恢复责任 |
| --- | --- | --- |
| `postgres-data` | `/var/lib/postgresql` | 数据库支持的一致备份、角色/扩展和恢复演练 |
| `gateway-spool` | `/var/lib/ai-gateway/request-log-spool` | 持久财务入口，保留完整目录，与数据库恢复点配套 |
| `gateway-plugins` | `/var/lib/ai-gateway/plugins` | 插件库存和制品，单独备份，不由数据库工具自动包含 |
| `gateway-image-edit-spool` | `/var/lib/ai-gateway/image-edit-spool` | 匿名短生命周期 scratch，不备份在途请求 |

entrypoint 以 root 读取只读挂载，将配置与秘密复制到私有 tmpfs，准备卷所有权，再降权为 UID/GID `10001`。遇到权限问题，先检查挂载和运行 UID，而不是扩大容器权限。根文件系统保持只读。

## 6. 停止与重新启动

停止前先在入口阻止新请求，让在途请求排空：

```bash
docker compose \
  --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml \
  stop gateway
```

`stop` 不删除持久卷。默认容器停止宽限为 3 分钟，应长于服务的请求宽限加日志排空时间；改大服务超时后同时调整部署停止预算。启动或应用配置仍使用前面的 `up -d --no-build`，不可用 `down -v` 做常规维护。

## 升级、备份与验收

升级时按[停写、备份与回滚流程](upgrades.md)保留原卷和配套恢复点。验收核对镜像身份、运行 UID、HTTPS 登录和 Cookie、渠道授权、日志与结算进度，以及停止后数据是否保留。实际发往上游的测试会产生费用。另为受保护的异机备份安排保留周期和恢复演练。
