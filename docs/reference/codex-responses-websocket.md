# Codex Responses WebSocket 实现参考

> 类型：外部实现参考，不是 `ai-gateway` 行为契约。
>
> 最近核对：2026-08-02；HTTP fallback 补充核对：2026-09-08（Codex CLI 0.130.0）、
> 2026-09-15（Codex CLI 0.154.0）；
> 连接状态恢复补充核对：2026-09-15（本地 `openai/codex@31ffe2bc9adccfe5fd3d29208250f796a13aa7a0`）。
>
> 参考版本：[`openai/codex@aa064463458adbef10400c74174107fc4b3550f0`](https://github.com/openai/codex/tree/aa064463458adbef10400c74174107fc4b3550f0)。
>
> 权威来源：[OpenAI WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)、[Codex Responses request type](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/common.rs)、[Codex Responses WebSocket endpoint](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/endpoint/responses_websocket.rs)、[Codex turn client](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/core/src/client.rs) 和 [Codex WebSocket dialer](https://github.com/openai/codex/tree/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/websocket-client/src)。

## 适用范围

先判断你在调查客户端恢复还是网关代理。本文研究的是页首固定版本的 Codex 客户端，
不是 OpenAI 规范，也不是要求所有 Gateway 客户端仿造 Codex Header。
“当前 Codex”仅指相应核对版本，不代表本次重新核验其最新发布。

| 问题 | 阅读位置 | 当前网关入口 |
| --- | --- | --- |
| 消息该怎么构造 | [response.create](#responsecreate-请求形状) | [Responses](responses.md) |
| 何时只发增量 | [预热与增量](#预连接预热与增量请求) | 精确连接状态，不由网关生成 delta |
| 为什么变为 HTTP | [错误与 fallback](#连接缓存错误与-fallback) | [客户端兼容测试](../development/real-upstream-smoke.md) |
| Codex OAuth 有什么额外规则 | [网关差异](#与-ai-gateway-的差异) | [插件接入](codex-oauth-connect.md) |

## 先看交互形状

下例只展示消息顺序，不是付费测试命令；model 必须换为可达客户端模型。
WS 握手使用 Gateway Bearer Key。具体 Codex provider 字段由插件约束。

```json
{"type":"response.create","model":"your-client-model","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Reply with OK."}]}]}
```

```text
建立连接
  -> 发完整 response.create
  -> 接收 Responses 事件，等待 response.completed
  -> 同连接发下一条（条件满足时 previous_response_id + 新增 input）
  -> 连接丢失：清增量状态，由客户端重发完整请求
```

网关不会把同一个 previous_response_id 送到任意新连接，也不在消息派发后代客户端重试。
下面的预连接、预热和增量判断是 Codex 客户端策略，不是网关自动替调用方执行的逻辑。

## `response.create` 请求形状

`ResponsesWsRequest` 只包含带 `type: "response.create"` 标签的
`ResponseCreateWsRequest`。后者直接复用普通 `ResponsesApiRequest` 的字段，并额外加入：

- `previous_response_id`；
- 预热使用的 `generate`；
- 每条请求更新的 `client_metadata`。

在所核对的 Codex commit 中，请求结构包含：

- `model`、`instructions`、`input`；
- `tools`、`tool_choice`、`parallel_tool_calls`；
- `reasoning`、`store`、`stream`、`stream_options`、`include`；
- `service_tier`、`prompt_cache_key`、`text`；
- `previous_response_id`、`generate`、`client_metadata`。

其中不包含 `max_output_tokens`。这不表示 OpenAI Responses API 全局禁止该字段，只表示 Codex
核对版本的 WebSocket 客户端不发送它。兼容上游可能在 HTTP 接受却在 WS 拒绝；
网关公共入口是否允许与 Codex 插件最终删除是两个独立层次，测试应明确验证所选层次。

`input` 中的普通消息会序列化为显式的 Responses item，例如：

```json
{
  "type": "message",
  "role": "user",
  "content": [
    {
      "type": "input_text",
      "text": "Reply with OK."
    }
  ]
}
```

Codex 正常请求使用 `stream: true`，并发送 `tool_choice`、`parallel_tool_calls`、`reasoning`、
`store` 和 `include` 等与 HTTP Responses 相同的非输入属性。

## 实现分层

| 层 | Codex 来源 | 责任 |
| --- | --- | --- |
| 请求模型 | `codex-api/src/common.rs` | 定义 `ResponsesApiRequest`、`ResponseCreateWsRequest` 和带 `type` 标签的 `ResponsesWsRequest`。 |
| 协议连接 | `codex-api/src/endpoint/responses_websocket.rs` | 握手、连接泵、顺序请求、事件解析、错误映射和连接关闭。 |
| Session 编排 | `core/src/client.rs` | Header、预连接/预热、增量请求、跨 turn 连接缓存和 HTTP fallback。 |
| 网络拨号 | `websocket-client/src/` | TLS、自定义 CA、代理、`NO_PROXY`、Happy Eyeballs 和 TCP_NODELAY。 |
| 行为测试 | `core/tests/suite/client_websockets.rs` | 固定握手 Header、请求 body、复用、错误、重连和 fallback 契约。 |

## 握手

Codex 从 provider 的 Responses URL 派生 WebSocket URL，再按以下优先级合并 Header：

1. provider 固定 Header；
2. 当前请求的额外 Header，覆盖 provider 同名值；
3. 默认 Header，只填充仍为空缺的名称；
4. 最后注入认证 Header。

正常 Codex 握手还会包含：

- `OpenAI-Beta: responses_websockets=2026-02-06`；
- `originator`；
- 以 thread ID 为值的 `x-client-request-id`；
- `session-id` 和 `thread-id`；
- User-Agent；
- 可选的 Codex compatibility、attestation 和 timing Header。

连接成功后，Codex 读取 `x-reasoning-included`、`x-models-etag`、`openai-model` 和
`x-codex-turn-state` 等响应 Header。握手探针还会短暂等待一个立即到达的 Close frame，以区分
“成功 Upgrade”与“Upgrade 后立刻被策略关闭”。

## 连接泵与顺序执行

Codex 把 Tungstenite stream 放入独立 pump task：

- 发送命令使用容量为 32 的有界 `mpsc`；
- 接收消息使用无界 channel；
- pump 负责回复 Ping、忽略 Pong，并把 Text/Binary/Close 转交协议层；
- pump task 随连接包装器销毁而 abort。

`ResponsesWebsocketConnection` 使用 `Mutex<Option<WsStream>>`，一个 response stream 在整个
生命周期内独占锁，因此同一 WebSocket 不会并发执行多个 `response.create`。向调用者发布
Responses 事件的 channel 容量为 1600。

发送和等待下一条事件都受 provider 的 stream idle timeout 约束。发生终态错误时，Codex 立即从
`Option` 取走并销毁 stream，不等待可能无限阻塞的 WebSocket Close handshake。

Codex 的 WebSocket 配置启用 per-message deflate。该扩展不是 OpenAI 协议的必选项，但部分
Codex 兼容上游在 Upgrade 后依赖该协商。`ai-gateway` 因此使用与所核对 Codex 版本相同、SHA
固定的 OpenAI Tungstenite forks，并在上游连接主动提供该扩展；上游未接受时仍使用未压缩消息。

## 预连接、预热与增量请求

Codex 区分两种提前工作：

- **preconnect**：只完成握手，不发送请求消息；
- **prewarm**：发送 `generate=false` 的 `response.create` 并等待
  `response.completed`。

一次成功响应后，Codex 保存完整上一请求、响应 ID 和服务端新增的 output items。只有满足以下条件时
才发送增量请求：

1. model、instructions、tools、tool choice、reasoning、store、stream、include、service tier、
   prompt cache key 和 text 等非输入属性保持一致；
2. 新 input 以“上一请求 input + 上一响应新增 items”为前缀；
3. 上一响应存在非空 response ID。

满足条件时，下一条消息只发送新增 input，并设置 `previous_response_id`。否则仍复用连接，但发送完整
请求且不设置 `previous_response_id`。`stream_options` 和 `client_metadata` 不参与上下文等价判断。

## 连接缓存、错误与 fallback

`ModelClient` 保存一个 Session 级 WebSocket 状态。创建 turn-scoped
`ModelClientSession` 时取出该状态，Session drop 时再放回，因此连接可以跨 turn 复用；每个 turn
仍创建独立的 `x-codex-turn-state` 容器。

主要恢复行为：

- 连接已关闭时重建连接，并清除增量请求状态；
- `websocket_connection_limit_reached` 和 `previous_response_not_found` 映射为可重试错误；
- `response.failed`、Close、I/O 错误或 idle timeout 会销毁当前连接；
- 握手返回 `426 Upgrade Required` 时立即切换 HTTP；
- WebSocket stream 重试预算耗尽后，对整个 Codex Session 永久启用 HTTP fallback；
- `401` 握手错误进入 Codex 的认证恢复流程。

这些重试发生在 Codex 客户端。服务端网关不能在已经发送 `response.create` 后再自动复制请求，否则
可能产生重复生成。

`previous_response_not_found` 的映射由
[`openai/codex@64dc1c7a01`](https://github.com/openai/codex/commit/64dc1c7a01b2aaa03701f08cf88b659c1a9737b3)
加入。Codex 在终态流错误后销毁当前 WebSocket；下一次连接建立会清除上一请求/响应的增量状态，因此
重试发送完整请求，而不是把同一个 `previous_response_id` 再发到新连接。

### Codex 0.130.0 的无可用 WS 路由兼容

补充来源：[客户端 fallback 分支](https://github.com/openai/codex/blob/rust-v0.130.0/codex-rs/core/src/client.rs)
和 [错误可重试分类](https://github.com/openai/codex/blob/rust-v0.130.0/codex-rs/protocol/src/error.rs)。
这次补充不更新上文基于固定 commit 的其他行为假设。

握手返回 HTTP 426 时，Codex 直接切换到 HTTP。升级后的错误帧即使包含 `status: 426`，
也不是握手拒绝；它按流传输错误消耗重试预算，再触发 Session 级 HTTP fallback。不能宣称给
任意 WS 错误增加 426 就能立即回退。503 被映射为不可重试的服务过载，因此不适合作为网关
“当前模型没有 WS 渠道”的回退提示。

网关在已授权范围内完全没有可选 WS 路由时拒绝握手；模型只能在首条消息中确定时，发送
`websocket_unavailable` 的 426 错误帧。实际 Codex CLI 0.130.0 已通过本地兼容测试：
两条路径都最终生成一次 HTTP 请求，且网关不做请求重放。命令和隔离条件见
[真实上游与客户端兼容测试](../development/real-upstream-smoke.md)；
网关错误契约见 [操作说明](../user/operations.md)。

## 代理和 TLS

Codex 的 `WebSocketConnector` 从共享 `HttpClientFactory` 解析目标路由，以保持 HTTP 与
WebSocket 的代理策略一致。拨号器覆盖：

- 直连与 transport 默认环境代理；
- 显式代理及 `NO_PROXY`；
- HTTPS proxy 的 TLS-to-proxy 和后续 tunnel；
- native roots 加 Codex 自定义 CA；
- 250 ms Happy Eyeballs；
- 可选 TCP_NODELAY。

代理配置或错误输出会隐藏代理 URL 中的敏感值。

## Codex 测试固定的行为

Codex WebSocket 测试至少覆盖：

- `OpenAI-Beta`、Session/thread/client-request ID 和 User-Agent；
- provider 声明支持 WebSocket 后无需额外 feature flag；
- preconnect、`generate=false` prewarm 和连接复用；
- Session drop 后继续复用同一连接；
- 握手 Header 变化不主动替换已经建立的连接；
- input 前缀匹配时使用 `previous_response_id` 和增量 items；
- 非输入属性变化或错误后恢复为完整请求；
- 连接 60 分钟限制错误后的重连；
- wrapped error event、rate limit、telemetry 和 HTTP fallback。

## 与 `ai-gateway` 的差异

| 方面 | 固定版本 Codex | 当前网关 |
| --- | --- | --- |
| 角色 | 最终客户端 | 服务端代理 |
| 请求 | 构造 Codex response.create | 公共入口策略、操作选路/别名/变换，然后按需交插件适配 |
| 增量 | 根据历史请求/响应生成 delta | 转发调用方增量，不自己生成 |
| 复用 | Session 持有连接 | 干净成功连接归有界、Session 隔离池 |
| 接收队列 | 核对实现无界 | 有界背压 |
| deflate | 核对版本启用 | 上游主动协商，未接受则未压缩 |
| 失败恢复 | 客户端重连/完整重发/fallback | 上游 Upgrade 前可换候选，发消息后不重试 |
| WS 能力 | provider 声明后使用 | 系统/用户条件与显式 responses-ws 操作能力、路由均须满足 |

### 连接和身份是宿主约束

当前池按 API Key、下游握手身份、渠道及凭证/接入版本、上游模型、插件 generation、目标、
网络策略和最终 Header 隔离。凭证轮换或改绑使旧身份失效，即使 Token 文本相同也不复用。
previous_response_id 缺失精确池连接时，返回 404 previous_response_not_found，
不是建新连接碰运气。

websocket_connection_limit_reached 和 previous_response_not_found 是状态恢复控制错误：
原形状保留，不执行事件 patch，废弃连接，不因这两类错误禁用渠道或清除 affinity。
客户端负责清状态、完整重发；网关不复制已发送的请求。

### Codex 专有准备属于外部插件

当前接入连接器为 codex，凭证子类型为 codex_oauth。没有内置 Codex fallback，
导入凭证也不自动创建 managed channel/projection；管理员显式绑定逻辑渠道及独立操作能力。

普通 JSON/Header 变换后，插件应用自己的 WS body/Header/隐私规则：
强制 stream=true/store=false，保留 previous_response_id/generate/client_metadata，
按契约删除 max_output_tokens 和显式 no-op，未知字段拒绝。
普通 general 不因这条规则删除 max_output_tokens。

插件补缺失身份和 prompt_cache_key，installation 使用凭证稳定 opaque UUID，
workspaces 使用插件设置的合成工作区。合成握手 turn metadata 不增加逐 turn 的随机身份，
以免破坏同 Session 池键。其余允许 metadata 保留；beta/routing/turn-state 等可按契约保留，
但插件不凭空制造这些声明。

插件改目标为 Codex base URL 下 responses，准备 OAuth、可选 account、FedRAMP 及
Codex identity；HTTP SSE 专用 Accept/Accept-Encoding/Content-Type 不进入 WS 握手。
不可变插件设置不属于宿主 system settings。最终 Header 与凭证目标仍受宿主 guard。

真实 smoke 的 Codex profile 可故意保留公共 max_output_tokens 以验证 provider policy，
不能要求测试自己先删除字段掩盖插件缺陷。
公共字段见[宿主契约](request-allowlists.json)，Codex 出站见
[插件仓库](https://github.com/oai404iao/ai-gateway-connectors)的
`codex/src/request-allowlists.json`；两份契约职责不同。

## 维护检查项

更新 Responses WebSocket 实现时重新核对：

1. Codex 的 `ResponseCreateWsRequest` 是否新增或删除字段；
2. Beta Header 值是否变化；
3. prewarm 与 `previous_response_id` 的连接级语义是否变化；
4. 连接限制错误码和 fallback 条件是否变化；
5. Header 合并优先级、代理和自定义 CA 行为是否变化；
6. Codex 是否仍启用 per-message deflate；
7. `ai-gateway` 的有界池、禁止多路复用和消息发送后不重试约束是否仍成立。

## 相关文档

- [Responses API 参考](responses.md)
- [OpenAI 兼容性总览](openai-compatibility.md)
- [真实上游 smoke test](../development/real-upstream-smoke.md)
- [当前架构](../development/architecture.md)
