# Codex Responses WebSocket：续写、中断与回退

> 类型：固定版本的客户端实现参考。
>
> 最近核对：2026-08-02（连接与增量），2026-09-08、2026-09-15（HTTP 回退），2026-11-19（状态丢失），2026-11-20（Lite、中断与消息大小）。
>
> 权威来源：[OpenAI WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)、[Codex 连接实现](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/endpoint/responses_websocket.rs)、[turn 客户端](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/core/src/client.rs)。

Codex 客户端决定何时发完整输入、何时发增量、何时切回 HTTP。网关转发客户端选择的请求；启用条件、上游连接池和网关错误帧见[使用 Responses WebSocket](../user/websocket.md)。

## 握手与第一条消息

客户端从 provider 的 Responses URL 派生 WS 地址，合并 provider 和请求 Header、填充默认值，最后注入认证。握手可包含 `OpenAI-Beta: responses_websockets=2026-02-06`、`session-id`、`thread-id`、`x-client-request-id`、`originator` 和 User-Agent。客户端协商 per-message deflate；握手本身不发起生成。

```json
{"type":"response.create","model":"your-client-model","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Reply with OK."}]}]}
```

每条连接同一时间只执行一个 `response.create`。`preconnect` 只建连接；`prewarm` 会发送 `generate:false` 的 create 并等待完成事件。请求类型见 [Responses wire types](https://github.com/openai/codex/blob/aa064463458adbef10400c74174107fc4b3550f0/codex-rs/codex-api/src/common.rs)。

[Lite 请求](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/core/src/client.rs#L1230-L1301)把指令消息和 `additional_tools` 放入 `input`。HTTP 用 `x-openai-internal-codex-responses-lite` 标识；WS 可只在每条消息的 `client_metadata.ws_request_header_x_openai_internal_codex_responses_lite` 标记。

## 何时发送增量

成功完成一轮后，客户端保存上一请求、响应 ID 以及服务端新增的 output items。若新输入以上一轮 input 和这些 output items 为前缀、模型及指令/工具等上下文属性相同，且响应 ID 非空，则同一连接上只发送新增 input 和 `previous_response_id`。`stream_options` 和 `client_metadata` 不参与上下文等价比较。其他情况下发送完整输入，即使仍使用同一连接。

## 中断与终态

拿到响应 ID 后，Lite 可发送 `response.interrupt`（`mode: "discard_partial_items"`）。Cancel 观察确认和终态；Drain 等待原响应的匹配终态。`response.interrupt.failed` 表示中断操作被拒，原响应仍可能正常完成。`response.incomplete` 的 `incomplete_details.reason` 为 `interrupted` 时，客户端按完成对象解析 ID 和 usage。见[控制帧](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/endpoint/responses_websocket.rs#L887-L993)、[终态解析](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/sse/responses.rs#L469-L542)、[Drain 行为测试](https://github.com/openai/codex/blob/rust-v0.171.0/codex-rs/core/tests/suite/pending_input.rs#L1590-L1739)。

## 客户端如何恢复

| 信号 | 固定版本的客户端处理 |
| --- | --- |
| 文本帧 `type:"error"`、`error.code:"previous_response_not_found"` | 按[错误码识别上下文丢失](https://github.com/openai/codex/blob/524e15989b79039e14d01cd197ed6e6580bd59f2/codex-rs/codex-api/src/endpoint/responses_websocket.rs#L768-L839)。丢弃失败连接；若重试预算允许，重建 WS，清除续写状态并[发送完整 input](https://github.com/openai/codex/blob/524e15989b79039e14d01cd197ed6e6580bd59f2/codex-rs/core/src/client.rs#L2492-L2570)。单独的 404 或 Close 不携带这种语义。 |
| `websocket_connection_limit_reached` | 丢弃连接，按重试预算重新建立会话上下文。 |
| 握手返回 HTTP `426` | [Codex 0.130.0](https://github.com/openai/codex/blob/rust-v0.130.0/codex-rs/core/src/client.rs) 直接改用 HTTP；Upgrade 成功后带 `status:426` 的错误帧则消耗流重试预算。 |
| 消息超限 | `x-codex-websocket-max-message-bytes` 给出服务端接收上限；若发送前已超限，或收到任何文本事件前服务端 Close 1009，客户端可回退 HTTP。见[帧与握手分类](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/endpoint/responses_websocket.rs)。 |

在网关处，消息一旦发给上游就不再自动重放。手动重发完整输入可能再次产生生成费用。
