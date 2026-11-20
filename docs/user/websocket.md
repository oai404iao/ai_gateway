# 使用 Responses WebSocket

> 状态：当前。

Responses WebSocket 把一条下游连接中的顺序 `response.create` 当作独立请求。增量状态依赖精确的上游连接，丢失状态时必须由客户端重发完整请求。

## 开启 WebSocket

WebSocket Upgrade 在 HTTP 握手阶段验证 Gateway API Key 和 Responses `proxy`
权限，但不消耗 RPM 或并发槽。该传输默认关闭，只有以下三层均开启才接受请求：

1. 管理员在 `/console/v1/system/settings` 中设置 `websocket.enabled = true`；
2. 用户在个人设置页 `/account/settings` 中开启 WebSocket，对应
   `GET/PUT /console/v1/me/settings` 的 `websocket_enabled`；
3. 管理员配置独立的 `responses-ws` 能力和同操作路由，API Key 已授权所属逻辑渠道；
   HTTP `responses` 能力和路由不能替代 WS 配置。

创建 Codex 凭证不会创建或启用能力、路由或渠道授权；需要管理员分别配置 Responses HTTP 与 WS。
系统与用户开关仍默认关闭。没有可配置的 `transports` 字段。
Chat Completions 渠道不能声明 WebSocket 支持。系统、用户未开启，或没有可用且声明支持的
Responses WS 路由时，HTTP Upgrade 统一返回 `426 websocket_unavailable`，提示客户端改用
`POST /v1/responses`，且不向客户端暴露系统开关、渠道或 Connector 凭证的内部状态：

- 握手时还没有请求模型。如果该 API Key 的全部已授权 Responses 路由都没有可选 WS 渠道，
  直接拒绝 Upgrade，返回 HTTP 426 和普通 JSON 错误体。
- 如果其他模型仍可使用 WS，允许 Upgrade；收到 `response.create` 后，若该模型没有可选
  WS 渠道，则发送 `type: "error"`、`status: 426`、
  `error.code: "websocket_unavailable"` 的错误帧并关闭连接。请求日志记录同样的状态和错误码。
- 未配置 WS 能力、渠道组/渠道停用、自动禁用，以及被动健康冷却或已被占用的半开探针都会影响
  可选性。明确不可用的 Codex Connector 凭证也不会使对应 projection 通过握手预检。预检不占用
  路由 lease、不推进权重轮转、不消耗 RPM/并发，也不发起上游请求；模型消息仍重新执行正常鉴权、
  准入和路由。
- 未知或无权访问的模型在消息阶段仍返回 `404 model_not_found`；Gateway API Key 鉴权/权限、
  请求校验、准入和进程关闭错误不会被改成 426。HTTP 无可选渠道仍使用
  `503 no_healthy_channel`。
- 上游渠道只能在首条消息后确定；若 Connector、目标配置、网络或上游 Upgrade 在发送
  `response.create` 前不可用，已经完成的下游 `101` 无法撤销，Gateway 改为发送同码的
  `426 websocket_unavailable` 错误帧并关闭连接。

握手 426 可供客户端改用 HTTP；升级后的 426 错误帧也由客户端决定是否及何时回退，**不保证立即回退**。Gateway 不把已发送的 WS 请求自动转换或重放为 HTTP。客户端实现与具体错误映射见
[Codex 参考](../reference/codex-responses-websocket.md)。

握手成功时，`x-codex-websocket-max-message-bytes` 告知客户端网关自身的单条消息字节上限，不代表上游限制。若上游随后在其握手中声明更小的上限，网关在派发前检查最终准备好的消息。超过任一限制，或上游在本次请求首条文本事件之前以 1009 拒绝消息时，网关用 Close 1009 通知客户端；收到文本事件之后的上游断开不作为大小回退信号。

管理员也可以在用户详情页修改同一个 `websocket_enabled` 个人偏好。该操作走版本化用户资源，
保存后立即重新发布数据面快照；用户仍可随后在自己的个人设置页再次调整。

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

如果终态先到，等待取消确认的客户端仍能收到本轮的元数据、计时事件和确认；持续无上游活动直到空闲超时，网关会关闭连接。如果中断与正常完成交错，网关已释放该轮上游连接，则对同一下游连接上刚完成且 ID 匹配的响应一次性答复 `response.interrupt.failed`，不再派发中断。已结束等待的客户端可以直接发下一条 create，包括用刚完成的 ID 精确续写。先前响应迟到的确认和相关元数据不会进入新请求的事件流、计量或空闲计时。响应 ID 冲突、无法区分或超过连接跟踪上限的中断会使连接不可复用。

## 继续上下文或恢复连接

**不带 `previous_response_id` 的完整请求不绑定上一个上游。** 每次 `response.create` 都按当前授权、优先级、权重与已配置的 Session affinity 选路；选好渠道/模型后才查找可复用的连接。同一下游连接可以先后使用不同上游，但同时只执行一个请求。

携带非空 `previous_response_id` 的请求依赖上游保存的上下文。网关只将其交给同一 API Key 和握手身份下、最近一次成功响应 ID 匹配的空闲连接，并再次检查当前授权、凭证、目标、网络策略和最终 Header。如果 Session affinity 已改绑到另一候选，续写仍使用符合路由条件的原连接，不把亲和记录改回去。

若找不到唯一匹配，或连接因空闲/总龄、容量、配置或凭证变化、进程重启或多实例漂移而丢失，网关不向新连接派发增量。它返回以下错误帧，由客户端清除增量状态并重发完整请求：

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

连接池只记每条连接最近成功响应的 ID 摘要，不保存对话历史，也不改写供应商返回的 ID。同一 Session 内的上游应生成唯一的响应 ID。如果旧记录已随新响应或连接淘汰而消失，供应商随后再使用同一 ID，单凭客户端的 ID 无法区分两次响应。不保证 ID 唯一的上游不宜混用同一 Session 的增量续写。

上游返回 `websocket_connection_limit_reached`（包括服务端 60 分钟连接上限）或
`previous_response_not_found` 时，Gateway 保留该控制错误帧、废弃对应上游连接，并允许客户端用完整
请求建立新连接。这两类状态恢复错误不会触发渠道自动禁用或清除 Session affinity；Gateway 自身仍不
重放已经发送的请求。

每条 WebSocket 连接同时只允许一个 `response.create` 在途。上游握手完成前的连接类失败仍可按全局
重试设置切换未尝试渠道；消息一旦发往上游，就不再自动重试，以避免重复生成。连接期间客户端 API Key
被撤销或过期后，下一条消息会收到 `invalid_api_key` 错误；系统或用户开关在连接期间关闭后，下一条
消息会收到 `426 websocket_unavailable` 并结束连接。

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
