# OpenAI Images API

> 类型：外部参考与项目兼容契约。
>
> 最近核对：2026-08-06。
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
- edit 使用独立的 Images edit 路径，并接收一个或多个输入图片；公开客户端通常使用
  `multipart/form-data` 上传 `image`/`image[]` 和可选 `mask`；
- 请求包含顶层模型名，其他 generation 字段由上游解释；
- GPT Image 模型直接返回 base64 图片数据，不支持 DALL-E 专用的 `response_format` 参数；
  `output_format` 缺省为 PNG；
- 响应是上游定义的 JSON；图片数据可能很大，网关不能为了检查输出而整体缓冲；
- 支持 usage 的上游可以在顶层返回输入和输出 token 统计；
- OpenAI 文档描述了图片 generation 的流式能力，但是否可用仍取决于模型和请求。

## ai-gateway 当前兼容行为

当前实现挂载：

```text
POST /v1/images/generations
POST /v1/images/edits
```

generation 路径：

- 使用独立的 `open_ai_images` 路由格式和 `images_generation` 请求操作；
- 只接受 JSON body，并执行与其他数据面相同的 Gateway API Key 鉴权、准入、模型路由、
  Header 清理、上游鉴权、被动健康和耐久请求日志；
- 校验 `images_generation.client_body` 顶层白名单，并提取路由所需的 `model` 和可选
  `stream`；policy 未删除字段且没有模型别名或请求 JSON 变换时保留原始请求字节；
- 下游 `Accept-Encoding` 不直接转发；网关独立向上游声明 gzip、deflate、Brotli 和
  Zstandard，流式解码支持的单层或多层响应 coding；
- 若上游返回顶层 `usage`，从解码后的流增量采集 `input_tokens`、`output_tokens` 及已支持的
  细分字段；
- 公共 listener 根据客户端 `Accept-Encoding` 独立重编码可压缩 JSON；已知小于 1KiB 时保持
  identity，长度未知时不为阈值判断缓冲可能包含 base64 图片的完整 JSON；
- 将请求日志记录为 `api_format = open_ai_images`、
  `api_operation = images_generation` 和 `request_protocol = non_stream`。

edit 路径：

- 使用相同的 `open_ai_images` 路由格式，但请求日志操作为 `images_edit`；
- 只接受带合法 boundary 的 `multipart/form-data`，要求一个非空 `model` 和至少一个
  `image`/`image[]` part；
- 顶层 multipart 字段必须进入 `images_edit.client_body` 白名单；未知字段返回本地 `400`。
  `moderation=auto` 是兼容通用表单的显式入口层忽略项，其他 moderation 值返回错误；
- 最多接受 16 张输入图片、一个 `mask` 和 64 个 part；每个文本字段最多 `64 KiB`，文本字段
  合计最多 `1 MiB`；
- boundary 最多 70 bytes；preamble、单个 part Header block 和 boundary padding 分别最多
  `8 KiB`、`16 KiB` 与 `1 KiB`，防止畸形 multipart framing 重新造成大内存缓冲；
- 默认总 body 上限为 `64 MiB`，单文件上限为 `50 MiB`。前 `1 MiB` 保存在内存，超过后写入
  受限目录中的匿名临时文件；
- 普通 OpenAI-compatible 渠道在模型不需要别名时原样回放 multipart；需要别名时流式等价重建
  multipart 并只替换 model part；
- 响应使用与 generation 相同的上下游 content-coding 协商和 Images usage collector，仍逐块
  转发解码或重编码后的流。

普通 OpenAI-compatible 渠道的模型名不在代码中硬编码；管理员必须配置 Images 渠道、可用上游
模型、计价模型和模型规则。Codex OAuth projection 是 provider-specific 例外，当前按核对的
Codex image tool 声明 `gpt-image-2`，管理员仍须创建对应本地模型与 Images model rule。

## 差异与限制

- edit 当前只接受 multipart，不接受 JSON/data URL 形式的公开客户端 edit 请求。
- 可选 MCP `image_gen.imagegen` 是独立 adapter：它只接受最多五个显式 PNG/JPEG/WebP
  base64 data URL，验证并逐块解码为上述 multipart 路径；这不会为公开
  `/v1/images/edits` 增加 JSON edit 契约。
- generation JSON 和 edit multipart 中的 `stream: true` 都返回本地
  `400 image_streaming_unsupported`，不会发送上游请求。
- Images generation 不使用自动上游重试或跨渠道故障转移；一次上游尝试开始后直接返回该尝试
  的结果，避免重复生成和重复计费；edit 使用相同边界。
- Images 渠道不能配置定时测试模型，不参与 paid scheduled probe。
- Images 不支持 Session 粘性、SSE 事件变换或 WebSocket。
- generation JSON 仍受全局 `request_limits.proxy_body_bytes` 限制；edit 使用独立的
  `image_edit_*` 限制和磁盘 spool，不提高默认 `1 MiB` JSON 内存 body limit。
- 普通 `openai_compatible` 与 Codex OAuth Images generation/edit 均已支持。Codex 凭证共享
  Token、可选 workspace/member、quota 和 outbound proxy，但使用独立的 Responses/Images group 与
  channel；Codex Images group 默认关闭，不会自动加入 API Key、Policy 或模型规则。
- Codex Images generation 会把目标改为 `/backend-api/codex/images/generations`，注入
  `x-codex-image-turn-id`，并删除 Responses 专用 session/thread Header。
- Codex Images edit 会把公共 multipart 流式转换为
  `/backend-api/codex/images/edits` 的 JSON `images[].image_url` data URL。该 provider
  仅接受最多五张图片、不接受 mask，并通过统一 Codex body 白名单拒绝无法等价表达的 edit
  字段；`output_format=png` 在 Codex 层删除，`moderation=auto` 已在客户端层删除。
- multipart edit 不执行请求 JSON Transform；配置了该类规则的选中渠道返回
  `image_edit_json_transform_unsupported`。Header 和响应 Header Transform 仍可使用。

Images generation/edit 的完整客户端和 Codex 字段动作见
[`request-allowlists.json`](request-allowlists.json)。

Codex 外部路径、Header 和当前 image model 的核对来源见
[Codex OAuth 与订阅后端接入参考](codex-oauth-connect.md)。

## 维护检查项

升级 Images 兼容范围时需要同时检查：

1. OpenAI generation、edit 和 streaming 官方契约是否变化；
2. `src/domain/api_format.rs`、`src/domain/api_operation.rs` 与公共路由；
3. Images body 大小、临时目录容量和 replay 策略，避免把 multipart/base64 请求整体放大到
   全局内存；
4. usage、请求日志、计费与重复生成风险；
5. Transform DSL 是否仍拒绝 Images SSE 规则；
6. deterministic proxy integration tests，以及可选付费真实上游 generation/edit smoke
   是否使用独立 URL、凭证和专用低额度图片模型。
