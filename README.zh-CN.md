# ai-gateway

中文 | [English](README.md)

`ai-gateway` 是一个单二进制 Rust 网关，用于转发 OpenAI 兼容的 LLM 请求。它向客户端提供 Chat Completions 与 Responses API，根据 PostgreSQL 控制面完成路由，并将请求转发到已配置的上游提供商。

文档已按读者分类整理，统一入口见[文档中心](docs/README.md)：用户文档、开发与
设计文档、OpenAI 外部参考和历史归档。

公共数据面和管理 Console API 是刻意分离的两个监听器：

- **数据面**（`/v1/*`）：面向客户端 API Key 与 OpenAI 兼容请求。
- **Console API**（`/console/v1/*`）：面向 JWT 认证的用户自助服务与管理员控制面管理。

## 特性

- **仅支持** OpenAI Chat Completions 和 Responses；两种格式绝不相互回退。
- 按 `(客户端模型名, API 格式)` 路由，支持渠道组优先级和渠道权重选择。
- 将 PostgreSQL 控制面记录编译为不可变内存快照，因此代理请求不需要逐次查询数据库。
- 可按配置执行模型别名、受限 JSON/Header/响应/SSE 变换。
- 转发前会移除客户端凭据和 hop-by-hop Header，再注入渠道专属的上游鉴权。
- 上游响应以流式方式转发，不缓冲完整响应；一旦发送响应头或任何响应字节，绝不重试或切换渠道。
- 提供进程内 RPM、并发和软额度准入控制、被动连接健康、异步请求日志、用量提取与结算。
- 提供独立的 JWT Console API，包括按用户邀请、可复用邀请码自助注册、轮换 refresh session、用户/管理员角色、审计日志，以及大多数可变资源的乐观并发控制。

## 架构

```text
OpenAI 兼容客户端
  │ Bearer API Key
  ▼
公共监听器（/v1/*）
  → 鉴权与准入
  → 不可变路由快照
  → 渠道选择与可选变换
  → 可复用 reqwest 上游客户端
  → 流式上游响应
  → 异步请求日志 / 用量 / 结算

Console 客户端
  │ JWT
  ▼
独立 Console 监听器（/console/v1/*）
  → 用户或管理员授权
  → PostgreSQL 控制面事务 + 审计记录
  → 立即发布运行时快照
```

## 环境要求

- Rust **1.92** 或更高版本（MSRV；Rust 2024 edition）。仓库通过
  `rust-toolchain.toml` 将日常开发和发布构建固定为 **1.97.1**。
- PostgreSQL
- Docker Compose（可选；`docker-compose.yml` 提供开发用 PostgreSQL，
  `docker-compose.prd.yaml` 可通过拉取或本地构建 Gateway 镜像运行完整生产栈）
- 建议安装 OpenSSL，用于生成本地数据库密码与 Console Ed25519 密钥

## 快速开始 / Quick start

> 默认配置文件为已被 Git 忽略的 `./config/config.toml`；本地 JWT 文件也统一放在 `./config/`。
> 服务不会加载 `.env` 文件，也不使用 XDG 配置目录。英文说明见 [README.md](README.md)。

### 1. 创建本地密钥与配置并启动 PostgreSQL

```bash
mkdir -p ./config
openssl rand -hex 32 > ./config/postgres-password
chmod 600 ./config/postgres-password
cp config.example.toml ./config/config.toml
docker compose up -d
```

按需编辑 `./config/config.toml`。至少确认 `[database]`、公共 `[server]` 和 `[upstream]` 超时设置。仓库内 Docker Compose 服务与示例数据库连接串相匹配。
示例通过 `./config/postgres-password` 读取数据库密码，不再在 TOML 或
Compose 中内置默认弱密码。Compose 默认值面向 4–8GiB 单节点主机，可通过带注释的
`AI_GATEWAY_POSTGRES_*` 环境变量覆盖。

如果从旧的根目录文件布局升级，请一次性迁移本地文件：

```bash
mkdir -p ./config
mv ./config.toml ./config/config.toml
mv ./console-jwt-private.pem ./console-jwt-public.pem ./config/
```

二进制会在启动时自动执行数据库 migration。

### 2. 启用 Console API（推荐）

Console API 是管理用户、API Key、模型、路由、渠道和变换的受支持方式。在当前工作目录的 `./config/` 中生成 Ed25519 密钥对；这些文件已被 Git 忽略：

```bash
openssl genpkey -algorithm Ed25519 \
  -out ./config/console-jwt-private.pem
openssl pkey \
  -in ./config/console-jwt-private.pem \
  -pubout \
  -out ./config/console-jwt-public.pem
chmod 600 ./config/console-jwt-private.pem
```

然后按 `config.example.toml` 中的注释模板，在 `./config/config.toml` 里启用 `[console]` 并填写 `[auth]`。例如，配置专用的 `127.0.0.1:3001` 监听器、明确的浏览器来源白名单，以及 `./config/` 下刚生成的两个 PEM 文件路径。

服务自身**不终止 TLS**。在向浏览器或互联网暴露 Console 监听器之前，必须将其置于正确配置的 HTTPS 反向代理之后。

### 3. 创建首个管理员

一次性 bootstrap 命令仅会在不存在 active admin 时成功。请从受保护的文件或密钥管理器通过标准输入传入密码：

```bash
cargo run -- bootstrap-admin \
  --email admin@example.com \
  --display-name "Initial Admin" \
  --password-stdin < /secure/path/admin-password.txt
```

### 4. 启动网关

```bash
cargo run
```

验证公共监听器：

```bash
curl -i http://127.0.0.1:3000/health
# HTTP/1.1 204 No Content
```

空控制面可以正常启动，但在创建用户/API Key 和路由配置前不能代理任何请求。

### 5. 配置控制面

使用 bootstrap 管理员登录 Console API，随后使用响应中的 access JWT 调用其他 Console 接口。如果你已启用 Console Web UI（`[console].ui_enabled = true` 且编译了 `embedded-console-ui` feature，或开发模式下的 Vite 开发服务器），直接浏览器打开 UI 即可，跳过下方 curl —— UI 调用的是同一套 Console API。

```bash
curl --request POST http://127.0.0.1:3001/console/v1/auth/login \
  --header 'Content-Type: application/json' \
  --data '{"email":"admin@example.com","password":"your-password"}'
```

要让数据面路由可用，请按以下顺序创建格式兼容的记录：

1. 一个带价格的**模型**。
2. 所需 API 格式的**渠道组**。
3. 该渠道组内的**渠道**：配置上游 URL、上游凭据和支持的上游模型名。
4. 一个**模型规则**：将客户端模型名映射到计价模型、上游模型名和路由目标。
5. 一个客户端 **API Key**：至少授予 `proxy` 权限；如需调用 `/v1/models`，还要授予 `models.read`。

即使使用同一个上游提供商或模型名，Chat Completions 路由与 Responses 路由仍是两套独立配置。请使用 Console API 管理控制面，不要直接编辑控制面数据表。

Console 路由覆盖与运行行为详见[运行与接口说明](docs/user/operations.md)。

## Docker 生产部署

发布镜像包含 Rust 二进制与嵌入式 Console UI。完整生产 Compose 默认不向宿主机
暴露 PostgreSQL，数据面与 Console 只绑定宿主机 loopback，并使用独立持久卷保存
request-log spool。

先准备已被 Git 忽略的配置与密钥：

```bash
mkdir -p ./config
cp deploy/compose/config.example.toml ./config/config.prd.toml
cp deploy/compose/env.example ./config/compose.prd.env
# 生成 ./config/postgres-password 和两个 Console JWT PEM 文件。
```

然后拉取固定版本镜像，或从当前 checkout 本地构建：

```bash
docker compose --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml pull gateway
# 或：docker compose --env-file ./config/compose.prd.env \
#       -f docker-compose.prd.yaml build gateway

docker compose --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml up -d --no-build
```

密钥生成、bootstrap-admin、反向代理/TLS、升级和备份边界详见
[Docker 生产部署说明](docs/user/production-deployment.md)。

## 手动转发性能测试

仓库提供一套必须显式启动、完全隔离的端到端转发性能 Harness。它会创建临时
PostgreSQL 数据库，启动 Mock LLM 上游和全新的 release Gateway 进程，执行直连与
Gateway JSON/SSE 负载，校验异步请求日志持久化率，并输出 Markdown/JSON 报告。

普通 `cargo test` 和 CI 不会执行该测试：

```bash
docker compose up -d
./scripts/run-forwarding-perf.sh --profile quick
```

完整设计、场景、安全边界和 `standard` 配置见
[转发性能测试设计与使用说明](docs/development/forwarding-performance.md)。

## 使用数据面

所有公共 API 接口均使用 `Authorization: Bearer <client-api-key>`。

| 接口 | 用途 |
| --- | --- |
| `GET /health` | 无需认证的存活检查；返回 `204`。 |
| `GET /v1/models` | 列出此 API Key 可达的模型；至少一个格式需要同时拥有 `proxy` 与 `models.read`。 |
| `POST /v1/chat/completions` | 仅代理 Chat Completions 请求。 |
| `POST /v1/responses` | 仅代理 Responses 请求。 |
| 带 WebSocket Upgrade 的 `GET /v1/responses` | 通过 WebSocket 顺序代理 Responses `response.create` 消息。 |

在创建匹配的模型规则和 API Key 后：

```bash
export GATEWAY_URL=http://127.0.0.1:3000
export AI_GATEWAY_API_KEY='replace-with-client-key'

curl "$GATEWAY_URL/v1/models" \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY"

curl --request POST "$GATEWAY_URL/v1/chat/completions" \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY" \
  --header 'Content-Type: application/json' \
  --data '{
    "model": "gateway-chat-model",
    "messages": [{"role": "user", "content": "Say hello."}]
  }'
```

对于 Responses，请先配置独立的 `open_ai_responses` 模型规则，再发送正常的 Responses 请求：

```bash
curl --request POST "$GATEWAY_URL/v1/responses" \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY" \
  --header 'Content-Type: application/json' \
  --data '{
    "model": "gateway-responses-model",
    "input": "Say hello."
  }'
```

网关会转发上游状态码和响应体，并保持流式行为；如客户端需要 SSE，请使用对应 OpenAI 请求中的流式字段。
Responses WebSocket 客户端使用同一个 Gateway Bearer Key 连接
`ws://<gateway>/v1/responses`（或通过终止 TLS 的反向代理使用 `wss://`）。网关对每个顺序到达的
`response.create` 独立执行准入和日志记录，在可用时取回同一 Session 隔离的上游 WebSocket 以保持
连接级缓存连续性，并在请求成功完成后把干净连接归还有界池；同一个 WebSocket 不支持并发多路复用
Responses 请求。WebSocket 转发默认关闭：管理员必须在数据库系统设置中启用总开关并把 Responses
渠道标记为支持，API Key 所属用户也必须在个人设置中启用；连接池容量和连接寿命同样由系统设置管理。
最小校验、透传、Streaming、重试和错误边界见
[OpenAI 兼容性参考](docs/reference/openai-compatibility.md)。

## 配置模型

TOML 仅保存进程级 bootstrap 配置。二进制默认读取
`./config/config.toml`；已忽略的 `./config/` 目录也保存本地 JWT 密钥文件。

| 区域 | 示例 |
| --- | --- |
| `[server]` | 公共监听器和优雅关闭期限。 |
| `[request_limits]` | 代理、Console 和认证接口各自独立的请求体大小限制。 |
| `[database]` | PostgreSQL URL、连接池大小和连接超时。 |
| `[upstream]` | 默认建连、响应头和流空闲超时。 |
| `[runtime_config]` | PostgreSQL 控制面定时重载间隔。 |
| `[passive_health]` | 连接失败阈值和冷却时间。 |
| `[request_logging]` | 本地耐久 spool、独立数据库池、COPY 入口、投影、结算与观测参数。 |
| `[console]` 与 `[auth]` | 可选的独立 Console 监听器与 JWT 密钥文件设置。 |

用户、API Key、模型、模型规则、渠道组、渠道、代理和变换模板等动态数据面配置保存在 PostgreSQL 中，并被编译为不可变运行时快照。项目刻意不支持 `[[api_keys]]`、`[[channels]]`、`[[model_rules]]` 之类的动态 TOML 表。

通过 Console API 写入配置时，服务会校验完整候选快照，并在提交后立即发布；定时重载器也会从 PostgreSQL 刷新快照。

终态请求日志会先跨过本地可恢复 spool，再通过 `COPY FROM` 进入低索引
PostgreSQL 入口表，随后异步投影和结算。耐久保证、故障边界与运维指标见
[docs/development/request-log-durability.md](docs/development/request-log-durability.md)。

生产机器规格分档、PostgreSQL 参数、密码文件、存储和容量验证方式见
[生产配置与容量调优](docs/user/production-configuration.md)。

## Console API

Console 监听器独立于公共监听器，使用短期 JWT access token。登录、邀请码注册、刷新和邀请激活成功时还会签发轮换的 `HttpOnly; Secure; SameSite=Lax` refresh Cookie。

- 用户自助接口位于 `/console/v1/me`，资源归属完全从 JWT 主体推导。
- 管理员可创建和调整可复用注册邀请码，配置可选次数、可选过期时间、目标用户组和初始 USD 额度；邀请码明文只在创建时返回。
- 仅管理员可调用的控制面接口可管理用户、注册邀请码、API Key Policy、API Key、模型、路由、网络代理、变换模板、请求日志、审计日志和手动重载。
- 大多数可变资源在 `GET` 时返回 `ETag`，并要求 `PUT` 携带 `If-Match` 实现乐观并发控制。
- `admin` 是用户角色，不是独立的 `/admin` API 命名空间，也不是进程级静态 Token。

完整路由清单见 [docs/user/operations.md](docs/user/operations.md)；Console 认证设计见 [docs/development/console-auth.md](docs/development/console-auth.md)。

## Console Web UI

Console 管理台是 **React + TypeScript + Vite + Tailwind CSS + shadcn/ui（Radix）**
单页应用，源码位于 `web/console/`。发布构建可将 Vite 静态产物嵌入 Rust 二进制，并只通过
独立的 Console listener 同源提供：

```text
https://console.example.com/                 # SPA
https://console.example.com/assets/*          # 静态资源
https://console.example.com/console/v1/*      # 既有 Console API
```

这不会把 UI 暴露到公共 `/v1/*` listener，也不引入 SSR 或常驻 Node 服务。Access token
只保存在浏览器内存；轮换 refresh cookie 继续保持 `HttpOnly; Secure; SameSite=Lax`。

UI 有两种运行模式。两者前置条件一致：PostgreSQL 已启动、`./config/config.toml` 已启用 `[console]` 并配好 `[auth]` JWT 密钥对（见快速启动步骤 1–2）、bootstrap 管理员已创建（步骤 3）。

### 运行模式 A — 开发模式（热重载，不嵌入）

分别运行网关（仅提供 Console API）和 Vite 开发服务器。开发服务器通过 HTTPS 提供 SPA，并将 `/console/v1/*` 代理到网关的 Console listener，因此你可以在热重载下编辑前端，同时由 Rust 二进制响应真实 API 调用。

```bash
# 终端 1 — 网关，公共监听 127.0.0.1:3000，Console 监听 127.0.0.1:3001
cargo run

# 终端 2 — Vite 开发服务器，https://console.localhost:5173
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console dev
```

浏览器打开 `https://console.localhost:5173`。`console.localhost` 主机和自签名 HTTPS 使 `__Host-` refresh cookie 及其 `Secure` 属性表现与生产一致。此模式下 `ui_enabled` 无意义（网关只提供 API）；Vite 开发服务器是开发用的 Node 进程，不是生产运行时。

### 运行模式 B — 生产模式（嵌入单二进制）

先构建前端，再用 `embedded-console-ui` feature 构建网关，Vite 产物会被烤进二进制并只从 Console listener 提供。生产环境没有 Node 进程。

```bash
# 1. 构建前端（生成 web/console/dist）
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build

# 2. 启用嵌入 feature 构建网关
cargo build --release --features embedded-console-ui
```

在 `./config/config.toml` 中设置 `[console].ui_enabled = true` 即可在 Console listener 挂载 SPA。若 `ui_enabled = true` 但未编译该 feature，启动会被拒绝。（debug 构建下 `rust-embed` 运行时从磁盘读取 `web/console/dist`，因此 dist 仍须存在；release 构建会将其嵌入。）

```bash
# 在 HTTPS 反向代理后运行单二进制
cargo run --release --features embedded-console-ui
# 浏览 Console listener origin，例如 https://console.example.com/
```

### 前端检查与测试

```bash
pnpm --dir web/console typecheck   # tsc --noEmit（严格）
pnpm --dir web/console lint         # oxlint
pnpm --dir web/console test         # vitest 组件测试（jsdom + MSW）
pnpm --dir web/console e2e          # Playwright 浏览器测试（会安装 Chromium）
pnpm --dir web/console generate:api:check   # OpenAPI spec/类型漂移门禁
```

完整命令列表、目录结构与 OpenAPI 契约流程见 `web/console/README.md`；目录布局、认证/缓存模型、shadcn 使用规范和分阶段实施计划见
[Console Web UI 设计与实施计划](docs/development/console-ui.md)。

## 运行行为与边界

- 请求在读取 body 前完成认证与准入。
- 只有模型别名或变换确有需要时才重新序列化请求；否则转发原始请求字节。
- 变换顺序固定为：模板默认值 → 渠道覆盖 → 受保护 Header 清理 → 上游鉴权。
- 客户端 `Authorization`、hop-by-hop Header 及 `Connection` 声明的 Header 永不转发给上游。
- 被动健康响应收到上游响应头之前的连接失败。配置允许时，只能在响应头前切换到
  尚未尝试的健康渠道；上游 HTTP 错误或已经开始的响应流不会重试。
- RPM、并发和软额度准入均为进程内状态；没有跨实例协调。
- 终态请求日志会先追加到本地 durable spool；用量提取和结算异步执行。额度是基于
  已结算用量的软预检查，不会在转发前预留本次成本。
- 请求日志不保存 prompt、completion、完整 Header、API Key、Cookie 或未经脱敏的上游错误内容。

当前范围不包含 embeddings、images、audio、files、batches、assistants、fine-tuning、通用自动重试、TLS 终止、独立财务账本、充值/退款或多币种兑换。

## 开发与验证

请在仓库根目录运行：

```bash
cargo check
cargo fmt --check
cargo clippy --all-targets
cargo test
```

发布准备与 tag 发布流程见 [`docs/development/releasing.md`](docs/development/releasing.md)。本地发布门禁：

```bash
./scripts/verify-release.sh 0.1.0
```

修改请求转发路径后，还必须运行可选的付费真实上游 smoke test。请使用低额度的专用凭据，并只将其保存在已忽略的本地密钥文件中：

```bash
cp .env.real-upstream.example .env.real-upstream
# 填写本地变量后运行：
./scripts/run-real-upstream-smoke.sh
```

该脚本是仓库中唯一允许使用 `.env` 的例外。运行前请阅读 [docs/development/real-upstream-smoke.md](docs/development/real-upstream-smoke.md)。

## 仓库结构

```text
src/
  application/       代理、Console 鉴权、控制面、日志、用量
  admission/         进程内 RPM、并发与软额度控制
  domain/            API 格式、路由、凭据、请求日志类型
  http/              公共 Axum 路由与独立 Console 路由
  persistence/       SQLx 仓储与 migration 集成
  runtime_config/    TOML bootstrap 配置与 ArcSwap 快照
  routing/           优先级/权重选择与被动健康
  transforms/        受限 JSON、Header、响应与 SSE 变换
  upstream/          可复用 reqwest 客户端与代理/超时策略
  workers/           快照重载与异步请求日志 worker
migrations/          PostgreSQL schema migration
deploy/postgres/     Compose 初始化辅助文件
docs/                用户、开发、外部参考与历史归档文档
config/              已忽略的运行配置、数据库密码和 JWT 密钥目录
config.example.toml  已跟踪的配置模板
tests/               本地与 PostgreSQL 集成测试
```

## 安全注意事项

- 将 JWT 私钥保存在已忽略的 `./config/` 本地文件中，并严格限制文件系统权限。
- 将 `./config/postgres-password` 权限保持为 `0600`，优先使用
  `[database].password_file`，不要把数据库密码直接写进 TOML。
- 请将数据库、备份和 Console 访问视为凭据敏感边界：控制面记录包含客户端和上游凭据。
- 不要将客户端/上游凭据或 JWT 私钥材料写入 TOML、源代码、日志、测试 fixture 或 shell 历史记录。
- Console API 仅应通过 HTTPS 和明确的来源策略对外暴露；公共数据面监听器同样应按网络边界限制访问。

## 文档

- [文档中心与文档规范](docs/README.md)
- [运行与接口说明](docs/user/operations.md)
- [生产配置与容量调优](docs/user/production-configuration.md)
- [Docker 生产部署](docs/user/production-deployment.md)
- [OpenAI 兼容性参考](docs/reference/openai-compatibility.md)
- [当前架构](docs/development/architecture.md)
- [版本发布流程](docs/development/releasing.md)
- [Console Web UI 设计与实施计划](docs/development/console-ui.md)
- [数据库与控制面设计](docs/development/database-design.md)
- [真实上游 smoke test 说明](docs/development/real-upstream-smoke.md)
- [产品与架构蓝图](docs/development/product-blueprint.md)

## 许可证

`ai-gateway` 采用
[GNU Affero General Public License v3.0 only](LICENSE)
（`AGPL-3.0-only`）许可证。

如果你修改本程序并通过网络向用户提供修改后的版本，AGPL 第 13 条要求向这些
用户提供获取对应源码的方式。第三方组件继续遵循各自的许可证；详见
[Console 第三方声明](web/console/NOTICES.md) 与仓库中的
[`LICENSES/`](LICENSES/)。
