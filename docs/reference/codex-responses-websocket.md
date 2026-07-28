# Codex Responses WebSocket 实现参考

> 类型：外部实现参考，不是 `ai-gateway` 行为契约。
>
> 最近核对：2026-07-27。
>
> 参考版本：[`openai/codex@fbe65995bbcd4da249cfdafe0300ac3cb2cb3b3c`](https://github.com/openai/codex/tree/fbe65995bbcd4da249cfdafe0300ac3cb2cb3b3c)。
>
> 权威来源：[OpenAI WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)、[Codex Responses WebSocket endpoint](https://github.com/openai/codex/blob/fbe65995bbcd4da249cfdafe0300ac3cb2cb3b3c/codex-rs/codex-api/src/endpoint/responses_websocket.rs)、[Codex turn client](https://github.com/openai/codex/blob/fbe65995bbcd4da249cfdafe0300ac3cb2cb3b3c/codex-rs/core/src/client.rs) 和 [Codex WebSocket dialer](https://github.com/openai/codex/blob/fbe65995bbcd4da249cfdafe0300ac3cb2cb3b3c/codex-rs/websocket-client/src/dialer.rs)。

## 适用范围

本文只整理 `ai-gateway` 实现与测试所依赖的 Codex Responses WebSocket
行为。Codex 是客户端实现，不是 OpenAI 协议规范；其内部 Header、遥测事件和预热机制不能自动成为
网关对所有客户端的承诺。

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

连接成功后，Codex读取 `x-reasoning-included`、`x-models-etag`、`openai-model` 和
`x-codex-turn-state` 等响应 Header。握手探针还会短暂等待一个立即到达的 Close frame，以区分
“成功 Upgrade”与“Upgrade 后立刻被策略关闭”。

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
当前 WebSocket 客户端不会发送它。兼容上游可能在 HTTP Responses 接受该字段，却在 Codex
WebSocket 入口拒绝它，因此真实 smoke 使用 Codex 形状而不额外添加输出上限字段。

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

Codex 的 WebSocket 配置启用 per-message deflate。该优化属于 Codex 客户端实现细节，不是
OpenAI 协议的必选项。

## 预连接、预热与增量请求

Codex 区分两种提前工作：

- **preconnect**：只完成握手，不发送请求消息；
- **prewarm**：发送 `generate=false` 的 `response.create` 并等待
  `response.completed`。

一次成功响应后，Codex保存完整上一请求、响应 ID 和服务端新增的 output items。只有满足以下条件时
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

| 方面 | Codex | `ai-gateway` |
| --- | --- | --- |
| 角色 | 最终客户端 | 透明服务端代理 |
| 请求构造 | 主动构造完整 Codex `response.create` | 只做最小 model 解析及已配置变换，不补齐 Codex body 字段 |
| 增量压缩 | 根据上一请求/响应自动生成 delta 和 `previous_response_id` | 原样转发客户端提供的增量语义 |
| 连接持有 | 一个 Session 缓存一个连接 | 成功请求后把干净连接归还到有界、Session 隔离的池 |
| 接收队列 | 无界 | 有界，向上游施加背压 |
| 压缩扩展 | 启用 per-message deflate | 当前不主动协商 per-message deflate |
| 失败恢复 | 客户端重连，预算耗尽后切 HTTP | 只允许上游 Upgrade 前故障转移；消息发送后不重试 |
| Header | Codex主动构造 Session 和内部 Header | 转发下游 Header，并按渠道变换/认证；缺省补 WebSocket Beta Header |
| 能力开关 | provider 声明支持后由 Codex 使用 | 网关要求系统、用户和 Responses 渠道三层均显式启用；默认关闭 |

因此，`ai-gateway` 不应为了某个兼容上游而全局删除 `max_output_tokens` 或伪造全部 Codex
metadata。需要模拟 Codex 的真实上游 smoke 应显式发送 Codex 请求形状；生产渠道的上游偏差应通过
渠道变换或上游专属配置处理。

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
