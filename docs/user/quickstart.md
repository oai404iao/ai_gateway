# 快速开始

> 状态：当前。以下步骤用于全新的本地 PostgreSQL 部署，不适用于覆盖已有配置或升级生产数据库。

本指南从源码启动网关，通过 Console API 配置一条通用 Chat Completions 路由，最后发送第一个代理请求。启动成功只说明进程可运行：空数据库没有上游、路由和有效 API Key，不能直接转发。

需要 Rust（仓库固定的 `rust-toolchain.toml`）、Docker Compose、OpenSSL、curl、jq 和 Bash。下列命令均从仓库根目录执行；首次构建需要下载依赖。只运行数据库和 Rust 二进制不需要 Node；浏览器管理界面见[启用 Console UI](#启用-console-ui)。如果希望直接使用容器，改读[生产部署](production-deployment.md)；Linux 文件后端改读 [SQLite](sqlite.md)。

## 1. 准备数据库与 Console 密钥

先确认 `config/` 中没有需要保留的同名文件；不要重新生成已有部署的数据库密码或 JWT 私钥。

```bash
umask 077
mkdir -p ./config ./data/plugins
cp config.example.toml ./config/config.toml
openssl rand -hex 32 > ./config/postgres-password
openssl genpkey -algorithm Ed25519 -out ./config/console-jwt-private.pem
openssl pkey -in ./config/console-jwt-private.pem -pubout \
  -out ./config/console-jwt-public.pem
docker compose up -d
docker compose ps
```

数据库应达到 healthy 状态。密码文件只初始化新数据卷；已有数据库的角色密码不会随文件替换而轮换。

编辑 `config/config.toml`：

1. 把 `[plugins].directory` 改为刚创建的 `data/plugins` 的**绝对路径**，可用 `pwd -P` 确认仓库路径。模板的 `/var/lib/ai-gateway/plugins` 需要预先授予运行用户权限，不适合直接由普通开发用户创建。
2. 保留模板的 PostgreSQL URL 与 `password_file`。
3. 用以下内容**替换**模板的 `[console]` 段，并启用 `[auth]` 段；不要添加重复的 TOML 表。

```toml
[console]
enabled = true
host = "127.0.0.1"
port = 3001
allowed_origins = []
ui_enabled = false

[auth]
issuer = "ai-gateway"
audience = "ai-gateway-console"
access_token_ttl_seconds = 900
refresh_token_ttl_seconds = 2_592_000
key_id = "primary-2026"
signing_key_path = "./config/console-jwt-private.pem"
verification_key_path = "./config/console-jwt-public.pem"
```

本地示例仅经 loopback 使用 curl，不开放远程明文 Console。浏览器和远程管理必须先配置可信 HTTPS 入口。服务不读取 `.env`；相对路径均以进程工作目录解析，不是 TOML 所在目录。

## 2. 创建首个管理员

先构建，再通过标准输入传入至少 12 字节的密码。下面的隐藏输入不会把密码写入命令历史；不要开启 shell `xtrace`。

```bash
cargo build --locked
read -r -s -p '管理员密码（至少 12 字节）: ' admin_password
printf '\n'
printf '%s' "$admin_password" | ./target/debug/ai-gateway bootstrap-admin \
  --email admin@example.com --display-name "Initial Admin" --password-stdin
unset admin_password
```

CLI 会运行 migration，仅在不存在 active admin 时创建首个管理员。密码遗失时使用[紧急恢复](security.md#紧急重置管理员密码)，不要反复运行 bootstrap 或改数据库哈希。

## 3. 启动并登录

在一个终端启动服务：

```bash
./target/debug/ai-gateway
```

在另一个 Bash 终端继续操作，仍从同一仓库根目录执行：

```bash
set -euo pipefail
curl --fail-with-body -i http://127.0.0.1:3000/health

console_url=http://127.0.0.1:3001/console/v1
read -r -s -p '管理员密码: ' admin_password
printf '\n'
session=$(printf '%s' "$admin_password" | jq -Rs \
  '{email:"admin@example.com",password:.}' | \
  curl --fail-with-body --silent --show-error "$console_url/auth/login" \
    -H 'Content-Type: application/json' --data-binary @-)
unset admin_password
access_token=$(printf '%s' "$session" | jq -er '.access_token')
user_id=$(printf '%s' "$session" | jq -er '.user.id')
unset session

post_console() {
  curl --fail-with-body --silent --show-error "$console_url/$1" \
    -H "Authorization: Bearer $access_token" \
    -H 'Content-Type: application/json' --data-binary @-
}
```

`/health` 返回 `204`；它不验证路由、上游或财务流水线就绪。下面使用短期 Access JWT，不存储 refresh Cookie。超过有效期后重新登录获取 token，再继续尚未执行的步骤；不要重复创建已经保存的资源。

## 4. 配置一条完整路由

下面通过管理员 API 为当前管理员创建一个演示 Key，不需要先配置普通用户 Policy。输入服务商的完整 Base URL、可用 wire model 和密钥。Base URL 的允许范围是**完整地址匹配**，不是域名或路径前缀；普通 OpenAI 兼容服务通常使用带 `/v1` 的 Base URL，实际以该服务为准。

```bash
read -r -p '上游 Base URL: ' upstream_base
read -r -p '上游模型名: ' upstream_model
read -r -s -p '上游 API Key: ' upstream_secret
printf '\n'

credential_id=$(printf '%s' "$upstream_secret" | jq -Rs --arg base "$upstream_base" \
  '{name:"quickstart",kind:"bearer",secret:.,allowed_base_urls:[$base],enabled:true}' | \
  post_console routing/upstream-credentials | jq -er '.id')
unset upstream_secret
access_id=$(jq -n --arg base "$upstream_base" \
  '{name:"quickstart",connector_kind:"general",base_url:$base,enabled:true}' | \
  post_console routing/accesses | jq -er '.id')
group_id=$(printf '%s' '{"name":"quickstart","enabled":true}' | \
  post_console routing/groups | jq -er '.id')
channel_id=$(jq -n --arg group "$group_id" --arg access "$access_id" \
  --arg credential "$credential_id" \
  '{name:"quickstart",group_id:$group,access_id:$access,credential_id:$credential,enabled:true}' | \
  post_console routing/logical-channels | jq -er '.id')
capability_id=$(jq -n --arg channel "$channel_id" --arg model "$upstream_model" \
  '{channel_id:$channel,settings:{operation:"chat_completion",enabled:true,
    available_models:[$model],request_compression:"default",test_model:null,
    test_pricing_model_id:null,auto_disable_allowed:false}}' | \
  post_console routing/capabilities | jq -er '.id')
```

再创建客户端模型 `gateway-demo` 与同操作候选。以下四类价格 `1 USD / 1,000,000 Token` **仅作演示记账**；请替换为实际价格后再承载业务，不能用它们推断上游账单。

```bash
model_id=$(jq -n --arg now "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{source_model_id:"gateway-demo",display_name:"Gateway demo",enabled:true,
    price_unit_tokens:1000000,input_unit_price:"1",cached_input_unit_price:"1",
    cache_write_unit_price:"1",output_unit_price:"1",price_effective_at:$now}' | \
  post_console models | jq -er '.id')
profile_id=$(jq -n --arg model "$model_id" '{model_id:$model}' | \
  post_console routing/profiles | jq -er '.id')
jq -n --arg profile "$profile_id" --arg capability "$capability_id" \
  --arg model "$upstream_model" \
  '{model_routing_profile_id:$profile,operation:"chat_completion",enabled:true,
    routing_tiers:[{priority:0,selection_strategy:"weighted_random",
      candidates:[{capability_id:$capability,upstream_model:$model,weight:1}]}]}' | \
  post_console routing/operation-rules
gateway_api_key=$(jq -n --arg user "$user_id" --arg channel "$channel_id" \
  '{user_id:$user,name:"quickstart",permissions:["proxy","models.read"],
    allowed_group_ids:[],allowed_channel_ids:[$channel]}' | \
  post_console api-keys | jq -er '.secret')
```

现在已经分别完成凭证、接入、管理组、逻辑渠道、操作能力、计价模型/profile、操作路由和 Key 授权。Responses、WS、搜索和两种 Images 操作需要各自的能力及规则，不会借用这条 Chat Completions 路由。详见[路由指南](routing.md)。

## 5. 发送第一个代理请求

先检查可见模型；下一条生成请求会访问真实上游，**可能产生费用**。

```bash
curl --fail-with-body http://127.0.0.1:3000/v1/models \
  -H "Authorization: Bearer $gateway_api_key"
curl --fail-with-body http://127.0.0.1:3000/v1/chat/completions \
  -H "Authorization: Bearer $gateway_api_key" \
  -H 'Content-Type: application/json' \
  --data '{"model":"gateway-demo","messages":[{"role":"user","content":"Reply hello."}],"stream":false}'
```

网关把 `gateway-demo` 改写为候选的 wire model，按客户端模型价格记账。费用异步结算；初始余额为零不代表不能继续产生费用，余额允许为负，Key 额度也只是[软预检查](billing.md)。

管理员可从 `GET /console/v1/request-logs` 和 `GET /console/v1/system/load` 核对结果。排障完成后退出当前 Console Session，并妥善保管或撤销演示 Key：

```bash
curl --fail-with-body --silent --show-error -X POST "$console_url/auth/logout" \
  -H "Authorization: Bearer $access_token"
unset access_token gateway_api_key
unset -f post_console
```

## 启用 Console UI

可选的浏览器界面需要 Node/pnpm（版本见 `web/console/package.json`）：

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build
cargo build --locked --release --features embedded-console-ui
```

停止开发进程，将 `[console].ui_enabled` 改为 `true`，再启动 `./target/release/ai-gateway`。界面仅在 `127.0.0.1:3001` 的 Console listener，不在数据面端口；生产浏览器访问前必须配置 HTTPS 反向代理。若 UI 与 Console API 不同源，再设置准确的 `allowed_origins`，不能用 `*` 代替。

## 遇到问题

| 现象 | 先检查 |
| --- | --- |
| 启动报插件目录权限错误 | 是否使用运行用户可管理、无符号链接的绝对私有目录 |
| 数据库认证失败 | 是否误用了已有 volume 的旧密码；不要删卷修复认证 |
| Console 401 | 管理员邮箱/密码、JWT 文件、Access JWT 是否过期 |
| `model_not_found` | 客户端模型名、同操作规则、Key 固定渠道授权 |
| `no_healthy_channel` | 接入、凭证、组、渠道和能力是否启用，目录和候选是否匹配 |
| `request_log_unavailable` | 本地 request spool 的容量、同步能力和错误锁定；不要删除证据 |

继续阅读[公共接口](data-plane.md)、[Console](console.md)、[日志与监控](observability.md)和[安全边界](security.md)。
