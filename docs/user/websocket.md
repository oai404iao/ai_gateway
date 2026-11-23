# 使用 Responses WebSocket

> 状态：当前。

Responses WebSocket 把一条下游连接中的顺序 `response.create` 当作独立请求。增量状态依赖精确的上游连接，丢失状态时必须由客户端重发完整请求。

## 开启 WebSocket

使用 Gateway API Key 对 `GET /v1/responses` 发起 Upgrade。握手检查 `proxy` 权限，不占用请求的 RPM 或并发槽。启用需要：

1. 管理员设置 `/console/v1/system/settings` 的 `websocket.enabled = true`。
2. 用户在个人设置开启 WebSocket（`/console/v1/me/settings` 的 `websocket_enabled`）。
3. 管理员配置 `responses-ws` 能力和路由，并授权所属逻辑渠道给调用方 Key。

传输未开启，或 Key 没有任何可用 WS 路由时，握手返回 HTTP `426 websocket_unavailable`。若收到模型名后才发现传输不可用，则发送同码错误帧并关闭连接；未知或未授权模型返回 `404 model_not_found`。客户端可改走 HTTP，前提是已配置该操作的路由。

下游握手响应 `x-codex-websocket-max-message-bytes` 声明网关的消息上限。网关也按上游握手声明的上限检查准备好的消息。超限或上游在首条文本事件前以 1009 拒绝消息时，下游收到 Close 1009。

## 发送与计量

每个后续 `response.create` 才作为一个独立逻辑请求：

- 重新检查 API Key 是否仍有效，执行 RPM、并发和软额度准入；
- 从消息顶层 `model` 完成 Responses 路由、模型别名和请求 JSON 变换；
- 应用渠道请求 Header 变换；若最终缺失则注入
  `OpenAI-Beta: responses_websockets=2026-02-06`，再应用上游认证，然后连接或复用上游
  WebSocket；
- 将上游 Responses JSON 事件逐消息转发，Responses SSE 事件变换规则也用于同类型
  WebSocket 事件；
- 在确认本轮终态后记录 usage、计费和请求日志。普通 Responses 将 `response.cancelled` 和
  `error` 视为终态；Codex 不以 `response.cancelled` 或未知的无状态码 `error` 通知结束本轮，
  具体分类见 [Responses 事件语义](../reference/responses.md#http-与事件)。

Responses Lite 可以仅在每条 create 的 `client_metadata.ws_request_header_x_openai_internal_codex_responses_lite` 中设置字符串 `"true"`，不需要额外的握手 Header。`input` 中的 `additional_tools` 和指令消息按请求内容转发，不转换成顶层 `tools` 或 `instructions`。

收到 `response.created` 后，客户端可针对其中的响应 ID 发送一次 `{"type":"response.interrupt","response_id":"…","mode":"discard_partial_items"}`。它使用本轮已绑定的上游连接，不另行计费。`response.interrupt.failed` 只表示中断未被接受，不替代原请求的终态。网关继续转发，直到匹配的终态到达；在此之前不能并发发送第二个 create。匹配的终态按上游 usage 结算并释放并发槽，不等待确认。Codex 的 `response.incomplete` 且原因是 `interrupted` 也可以是成功终态。

若终态先于中断确认到达，可继续等待确认（受空闲超时限制），也可用刚完成的响应 ID 发下一条 create。若中断到达网关时，上游连接已释放，网关会对刚完成且 ID 匹配的响应一次性答复 `response.interrupt.failed`。上一轮迟到的确认不会混入下一轮。

## 继续上下文或恢复连接

**不带 `previous_response_id` 的完整请求不绑定上一个上游。** 每次 `response.create` 都按当前授权、优先级、权重与已配置的 Session affinity 选路；选好渠道/模型后才查找可复用的连接。同一下游连接可以先后使用不同上游，但同时只执行一个请求。

携带非空 `previous_response_id` 的请求依赖上游保存的上下文。网关只将其交给同一 API Key 和握手身份下、最近一次成功响应 ID 匹配的空闲连接，并再次检查当前授权、凭证、目标、网络策略和最终 Header。如果 Session affinity 已改绑到另一候选，续写仍使用符合路由条件的原连接，不把亲和记录改回去。

若找不到唯一匹配（例如空闲连接过期、轮换密钥、进程重启或切到其他实例），网关不向新连接派发增量。收到以下错误帧后，客户端需清除增量状态，重新发送完整输入：

```json
{
  "type": "error",
  "status": 404,
  "error": {
    "type": "invalid_request_error",
    "code": "previous_response_not_found",
    "message": "Previous response was not found. Retrying the full request."
  }
}
```

连接池只记每个连接最近成功响应的 ID 摘要，不保存对话内容。一个 Session 内的上游需生成唯一的响应 ID；ID 重用时，客户端无法仅凭该 ID 区分两次响应。

上游返回 `websocket_connection_limit_reached` 或 `previous_response_not_found` 时，网关原样转发，废弃连接，不禁用渠道或清除 Session affinity。只有消息发出前的建连故障可按策略换候选；消息发出后不自动重试。会话中撤销 Key 或关闭 WS 时，下一条 create 会收到相应的 `invalid_api_key` 或 `websocket_unavailable`。

## 配置反向代理与连接池

由于上游渠道只能在下游 Upgrade 完成并收到首条 `response.create` 后确定，上游 Upgrade 响应 Header
无法回填到已经完成的下游握手；配置的响应 Header 变换因此只适用于 HTTP Responses，WebSocket
事件变换仍正常生效。HTTP、HTTPS、SOCKS4/4a 和 SOCKS5/5h 渠道代理策略均用于上游 WebSocket 建连。
若公共 listener 前有 TLS/负载均衡反向代理，必须允许 WebSocket Upgrade，并把连接空闲和最长时限
设置得足以覆盖模型响应；网关自身仍不终止 TLS。

Codex 会在握手中发送 `session-id`、`thread-id`、`x-client-request-id`、`originator` 和
User-Agent。网关会把这些非 hop-by-hop Header 纳入连接池隔离并转发；反向代理和渠道 Header 变换
不应无意删除它们。Codex 请求形状与恢复逻辑见
[Codex Responses WebSocket 实现参考](../reference/codex-responses-websocket.md)。

系统设置中的 WebSocket 连接池参数为：

- `max_idle_connections`：进程级最大空闲上游连接数，范围 `0..=4096`，默认 `128`；`0`
  表示保留 WebSocket 转发但不复用空闲连接。
- `idle_timeout_seconds`：空闲连接保留时间，范围 `1..=3600`，默认 `300`。
- `max_connection_age_seconds`：连接总寿命，范围 `60..=3600`，默认 `3300`，且必须大于
  空闲超时。

## 停机排空

进程收到关闭信号后不再接受新的 WebSocket Upgrade，立即清空空闲上游池并用 `1001` 关闭空闲下游
连接；已经在途的 `response.create` 可在 `shutdown_grace_period_seconds` 内完成，超过时限后会被
强制取消并按客户端取消记录。
