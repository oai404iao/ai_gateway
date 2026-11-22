# 快速开始

> 状态：当前。

本例从源码启动网关，在终端把客户端模型 `gateway-demo` 接到你自己的 Chat Completions 上游。
全程用 curl 调用 Console API，无需构建浏览器界面。

准备 Bash、[仓库指定的 Rust 工具链](../../rust-toolchain.toml)、Docker Compose、OpenSSL、curl、jq，以及上游的 Base URL、模型 ID 和 API Key。以下命令均从仓库根目录执行。使用发行镜像请看 [Compose 部署](production-deployment.md)，使用 Linux 文件数据库请看 [SQLite 部署](sqlite.md)。

## 1. 准备数据库与 Console 密钥

以下命令只用于**新部署**，会写入 `config/` 下的配置、数据库密码和 JWT 密钥。已有部署请走[升级流程](upgrades.md)，替换密码文件不会修改已有数据库的密码。

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

编辑刚复制的 `config/config.toml`，保留 `[database]` 的示例值：

1. 将 `[plugins]` 的 `directory` 设为 `data/plugins` 的绝对路径（用 `pwd -P` 查看仓库路径）。
2. 在 `[console]` 中改为 `enabled = true`，取消 `host = "127.0.0.1"` 和 `port = 3001` 两行的注释。本例只从本机访问。
3. 取消 `[auth]` 及该段各配置项的注释。示例中的两个 PEM 文件路径指向上一步生成的密钥。

等 `docker compose ps` 中的 `postgres` 显示 `healthy`，再创建账户。

## 2. 创建首个管理员

构建后，用标准输入设置密码；输入时终端不会回显。

```bash
cargo build --locked
set +x
IFS= read -r -s -p '管理员密码（至少 12 字节）: ' admin_password
printf '\n'
printf '%s' "$admin_password" | ./target/debug/ai-gateway bootstrap-admin \
  --email admin@example.com --display-name "Initial Admin" --password-stdin
unset admin_password
```

看到 `bootstrap administrator created` 后即可启动服务。已有管理员忘记密码时，请用[管理员密码重置](security.md#紧急重置管理员密码)。

## 3. 启动并登录

在一个终端启动服务：

```bash
./target/debug/ai-gateway
```

在第二个 Bash 终端检查公共端口，再登录独立的 Console 端口。后续命令都在这个终端执行：

```bash
set -euo pipefail
set +x
curl --fail-with-body -i http://127.0.0.1:3000/health

console_url=http://127.0.0.1:3001/console/v1
IFS= read -r -s -p '管理员密码: ' admin_password
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

`/health` 的 `204` 表示进程可达。`post_console` 用刚取得的短期登录令牌管理路由，此时客户端还没有可调用的模型。

## 4. 配置一条完整路由

**接入上游。** 输入服务商给出的完整 Base URL（包括服务商要求的 `/v1` 等路径，但不含 `/chat/completions`）。下面把凭证限定在该地址，把接入和凭证绑定到一个逻辑渠道，并登记它能处理的操作和模型。

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

**发布客户端模型。** 将 `gateway-demo` 的 Chat Completions 请求路由到上一步的上游模型。示例的四项单价均为每百万 token 1 美元，只用于网关演示记账；实际使用时填自己的价格。

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
```

**授权调用。** 为当前管理员创建一个网关 API Key，授权它查看模型并通过这个渠道转发请求。客户端用这个 Key，而不是服务商的密钥或管理员登录令牌。

```bash
gateway_api_key=$(jq -n --arg user "$user_id" --arg channel "$channel_id" \
  '{user_id:$user,name:"quickstart",permissions:["proxy","models.read"],
    allowed_group_ids:[],allowed_channel_ids:[$channel]}' | \
  post_console api-keys | jq -er '.secret')
```

## 5. 发送第一个代理请求

先确认模型列表中有 `gateway-demo`：

```bash
curl --fail-with-body http://127.0.0.1:3000/v1/models \
  -H "Authorization: Bearer $gateway_api_key" | jq -r '.data[].id'
```

再生成一条回复。这次请求会调用真实上游，**可能产生费用**；网关的[软额度](billing.md)不是费用上限。

```bash
curl --fail-with-body http://127.0.0.1:3000/v1/chat/completions \
  -H "Authorization: Bearer $gateway_api_key" \
  -H 'Content-Type: application/json' \
  --data '{"model":"gateway-demo","messages":[{"role":"user","content":"Reply hello."}],"stream":false}'
```

响应中的 `choices[0].message.content` 是上游模型的回答。可在 [Console 请求日志](observability.md)核对客户端模型、实际使用的上游模型及用量。

结束后退出 Console。退出登录不会撤销网关 Key；按需妥善保存，或在 [Console](console.md) 撤销演示 Key。

```bash
curl --fail-with-body --silent --show-error -X POST "$console_url/auth/logout" \
  -H "Authorization: Bearer $access_token"
unset access_token gateway_api_key
unset -f post_console
```

## 遇到问题

| 现象 | 先检查 |
| --- | --- |
| 启动报插件目录权限错误 | 是否使用运行用户可管理、无符号链接的绝对私有目录 |
| 数据库认证失败 | 确认密码文件对应当前数据卷；保留已有数据卷 |
| 登录后调用 Console 返回 `401` | 重新执行登录部分获取新令牌，再从未完成的步骤继续 |
| `model_not_found` | 客户端模型名、同操作规则、Key 是否授权候选渠道或其当前组 |
| `no_healthy_channel` | 接入、凭证、组、渠道和能力是否启用，目录和候选是否匹配 |
| `request_log_unavailable` | 按[日志与监控](observability.md)检查本地 spool 容量和写入错误，保留未结算记录 |

需要浏览器管理界面时，按 [Console UI 构建说明](../../web/console/README.md)启用嵌入式界面，并为 Console 配置 HTTPS 反向代理。要接入更多模型或操作，见[路由指南](routing.md)。
