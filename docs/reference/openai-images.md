# OpenAI Images API

> 类型：外部参考与项目兼容契约。
>
> 最近核对：2026-07-31。
>
> 权威来源：
> [OpenAI Images API Reference](https://developers.openai.com/api/reference/resources/images)、
> [OpenAI Image generation guide](https://developers.openai.com/api/docs/guides/image-generation)、
> [OpenAI Images streaming guide](https://developers.openai.com/api/docs/guides/image-generation#streaming)。

## 外部接口关键语义

OpenAI Images API 将图片生成和图片编辑作为独立操作。请求与响应字段、支持模型、输出编码、
质量、尺寸和流式事件会随官方接口演进；调用方应以官方 API Reference 为准。

本项目只依赖以下稳定边界：

- generation 使用独立的 Images 路径，而不是 Chat Completions 或 Responses 路径；
- 请求包含顶层模型名，其他 generation 字段由上游解释；
- 响应是上游定义的 JSON；图片数据可能很大，网关不能为了检查输出而整体缓冲；
- 支持 usage 的上游可以在顶层返回输入和输出 token 统计；
- OpenAI 文档描述了图片 generation 的流式能力，但是否可用仍取决于模型和请求。

## ai-gateway 当前兼容行为

当前实现只挂载：

```text
POST /v1/images/generations
```

该路径：

- 使用独立的 `open_ai_images` 路由格式和 `images_generation` 请求操作；
- 只接受 JSON body，并执行与其他数据面相同的 Gateway API Key 鉴权、准入、模型路由、
  Header 清理、上游鉴权、被动健康和耐久请求日志；
- 只解析路由所需的顶层 `model` 和可选 `stream`，没有模型别名或请求 JSON 变换时保留原始
  请求字节；
- 默认逐块透传上游响应，不缓冲可能包含 base64 图片的完整 JSON；
- 若上游返回顶层 `usage`，增量采集 `input_tokens`、`output_tokens` 及已支持的细分字段；
- 将请求日志记录为 `api_format = open_ai_images`、
  `api_operation = images_generation` 和 `request_protocol = non_stream`。

普通 OpenAI-compatible 渠道的模型名不在代码中硬编码；管理员必须配置 Images 渠道、可用上游
模型、计价模型和模型规则。Codex OAuth projection 是 provider-specific 例外，当前按核对的
Codex image tool 声明 `gpt-image-2`，管理员仍须创建对应本地模型与 Images model rule。

## 差异与限制

- 不挂载 `POST /v1/images/edits`。
- 不接受 multipart body 或客户端图片上传。
- `stream: true` 返回本地 `400 image_streaming_unsupported`，不会发送上游请求。
- Images generation 不使用自动上游重试或跨渠道故障转移；一次上游尝试开始后直接返回该尝试
  的结果，避免重复生成和重复计费。
- Images 渠道不能配置定时测试模型，不参与 paid scheduled probe。
- Images 不支持 Session 粘性、SSE 事件变换或 WebSocket。
- generation JSON 仍受全局 `request_limits.proxy_body_bytes` 限制；首期没有提高默认
  `1 MiB` 内存 body limit。
- 普通 `openai_compatible` 与 Codex OAuth Images generation 均已支持。Codex 凭证共享 Token、
  workspace/member、quota 和 outbound proxy，但使用独立的 Responses/Images group 与 channel；
  Codex Images group 默认关闭，不会自动加入 API Key、Policy 或模型规则。
- Codex Images generation 会把目标改为 `/backend-api/codex/images/generations`，注入
  `x-codex-image-turn-id`，并删除 Responses 专用 session/thread Header。

Codex 外部路径、Header 和当前 image model 的核对来源见
[Codex OAuth 与订阅后端接入参考](codex-oauth-connect.md)。

## 维护检查项

升级 Images 兼容范围时需要同时检查：

1. OpenAI generation、edit 和 streaming 官方契约是否变化；
2. `src/domain/api_format.rs`、`src/domain/api_operation.rs` 与公共路由；
3. Images body 大小和 replay 策略，避免把 multipart/base64 请求整体放大到全局内存；
4. usage、请求日志、计费与重复生成风险；
5. Transform DSL 是否仍拒绝 Images SSE 规则；
6. deterministic proxy integration tests，以及付费真实上游 smoke 是否具备专用低额度图片模型。
