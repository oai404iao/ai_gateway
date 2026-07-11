### 项目描述

一个 基于RUST的 高性能 LLM请求转发 网关.

### 技术栈

- HTTP：axum + tower + tower-http
- 上游请求：reqwest（rustls-tls、stream）
- 异步运行时：tokio
- 数据库：PostgreSQL + sqlx
- 配置热加载：arc-swap
- JSON：serde_json；受限的 JSON Patch / JSON Pointer
- 日志与指标：tracing、（OpenTelemetry、Prometheus   这俩暂不实现)
- 密钥：secrecy、zeroize
- 金额：rust_decimal

### 架构

模块化单体:  一个 Rust 服务，明确分离数据面（请求转发）与控制面（配置、模型同步、日志），暂不引入微服务或 Redis。

Axum HTTP
  → 鉴权
  → 路由格式识别
  → 模型规则解析
  → 渠道组选路 / 负载均衡
  → 请求变换 + 上游鉴权注入
  → reqwest 转发
  → 响应直通 / 响应变换
  → 异步日志与用量结算

### 实体

```
User
 └─ API Key

Channel Group
 └─ Channel
     ├─ Proxy
     └─ Config Template
```

| 实体                 | 关键职责                                                                           |
| ------------------ | ------------------------------------------------------------------------------ |
| `users`            | 控制台用户或租户, 余额                                                                   |
| `api_keys`         | 客户端 Bearer Key；存 原始内容、状态、过期时间、可用分组、限制额度                                        |
| `models`           | 从 models.dev 同步的标准模型目录, 价格也包括在这里(输入、缓存输入、缓存写入、输出)                              |
| `model_rules`      | `(客户端模型名, API格式) -> 渠道组(可多选)/渠道(可多选) + 上游模型名`,  系统级别的路由                        |
| `channel_groups`   | 同一种协议格式的负载均衡池, 有优先级                                                            |
| `channels`         | 实际上游地址、原始上游鉴权、权重、超时、请求头配置、请求体配置、响应体配置、状态、可用模型列表、测试配置(测试模型, 是否允许被自动禁用, 测试请求体配置) |
| `proxies`          | HTTP/SOCKS 出口代理配置                                                              |
| `config_templates` | 可复用的请求/响应变换和网络配置                                                               |
| `request_logs`     | 请求、用户、KEY 、渠道、延迟(TTFT和TPS)、用量、费用、失败原因、本次请求采用的单价快照                              |
| `audit_logs`       | 控制面配置变更记录                                                                      |
| `system_settings`  | 系统配置, 存储JSONB.  重试(重试次数, 自动禁用配置) , 超时, 是否可以注册， 定时测试 等配置                        |

ps: 客户端APIKEY和上游APIKEY 都明文存储, 因为只有管理员和用户才接触的到。本项目要求用户和管理员可以看见KEY.

关键约束：

```text
UNIQUE(client_model, api_format) ON model_rules

model_rule.api_format == channel_group.api_format
channel_group.api_format == channel.api_format
```

`api_format` 只允许：

```rust
enum ApiFormat {
    OpenAiChatCompletions,
    OpenAiResponses,
}
```

不设计成“万能 Provider 抽象”。两个格式共用选路和转发能力，但各自保持独立的路径和校验规则，禁止跨格式回退或转换。

## 请求处理

`/v1/chat/completions` 与 `/v1/responses` 都调用同一个 `proxy(format)` 用例：

1. 验证客户端 API Key。
2. 在受限大小内读取原始 JSON 请求体，只提取 `model`。
3. 根据 `(format, model)` 找到启用的 `model_rule`。
4. 校验渠道组存在、启用，且格式一致。
5. 从健康渠道中按优先级、权重选择一个渠道。
6. 应用模板和渠道配置中的请求变换。
7. 移除客户端鉴权，最后注入渠道的上游鉴权。
8. 原路径转发给上游。
9. 直通或按配置变换响应。
10. 异步记录日志、Token 用量和成本。

默认请求体应保持原始字节不变。即使为了读取 `model` 而解析过 JSON，只要没有配置变换或模型别名映射，也不要重新序列化。

模型别名是唯一默认允许的请求体修改：

```text
客户端：gpt-4.1
上游：gpt-4.1-2025-04-14
```

## 高级变换配置

不要支持任意 JavaScript、Shell 或模板执行。应采用受限、可编译、可验证的 DSL：

- 请求头：`set`、`remove`、`rename`
- JSON 请求体：受限 JSON Pointer / JSON Patch
- 非流式 JSON 响应：受限 JSON Patch
- 流式 SSE 响应：仅支持逐个 `data:` JSON 事件变换

变换顺序固定：

```text
模板默认值 → 渠道覆盖 → 上游鉴权注入
```

以下头必须受保护，配置不能修改：

```text
Host
Content-Length
Connection
Transfer-Encoding
Authorization（客户端）
Proxy-Authorization
```

同时必须剥离所有 hop-by-hop headers，以及 `Connection` 中动态声明的头。流式响应若经过变换，不能沿用原始 `Content-Length`。

## Streaming 策略

- 请求体：读取并限制大小，因为必须识别 `model`。
- 上游响应：使用 `reqwest_response.bytes_stream()` 直接转成 Axum `Body`，不缓冲。
- 客户端断开时，丢弃流以取消上游请求。
- 一旦响应头或首个流块已经发送，绝不能切换渠道或重试。
- 非流式响应变换可以完整缓冲 JSON；普通流式响应绝不能为变换而整体缓冲。
- SSE 变换必须按事件边界解析，不能按网络 chunk 处理。

超时应区分：

- 建连超时；
- 等待响应头超时；
- 流空闲超时；
- 不设置一个会杀死长生成请求的“总响应超时”。

## 渠道与负载均衡

```text
优先级 → 同优先级中按权重随机/轮询 → 健康检查过滤
```

渠道状态分为：

- 管理状态：启用 / 禁用
- 运行状态：健康 / 熔断中 / 冷却中
- 运行指标：失败次数、延迟、in-flight 请求数

生成请求的自动重试风险很高，可能重复扣费。只允许在“尚未收到上游任何响应字节”时，对连接失败等按配置重试。

代理是 `reqwest::Client` 级配置，因此不要每次请求新建 Client。建立一个按“代理、TLS、超时策略”索引的 `UpstreamClientRegistry`，复用连接池。

## 配置与 models.dev

数据库配置不应在每个请求中实时查询。控制面变更后，把有效配置编译为不可变快照：

```text
(api_format, client_model)
  → CompiledModelRule
  → CompiledChannelGroup
  → CompiledChannels
```

通过 `ArcSwap` 原子替换运行时快照。只考虑单实例

`models.dev` 由管理员与后台手动同步并选择模型 写入 `models`，绝不在请求链路实时访问。价格必须带生效时间；日志中保存当时使用的价格快照，避免历史费用被后续价格更新篡改。

`/v1/models` 不应返回全局模型目录，而是返回当前 API Key 允许访问的 `model_rules.client_model`，并输出 OpenAI 兼容的列表格式。

## 代码组织

保持一个可部署二进制，但按边界分模块:

```text
src/
  http/            # Axum 路由、错误响应、中间件
  application/     # ProxyUseCase、模型查询、配置管理
  domain/          # 实体、值对象、规则和接口
  routing/         # 模型规则、渠道选择、健康状态
  transforms/      # 编译与执行受限变换
  upstream/        # reqwest、代理、鉴权、Header 清理
  persistence/     # SQLx repository、迁移
  runtime_config/  # 配置快照与热更新
  observability/   # tracing、metrics、请求日志
  workers/         # models.dev 同步、日志落库、健康检查
migrations/
```

---
