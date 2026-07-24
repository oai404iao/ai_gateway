# Chat Completions 参考

> 类型：外部参考。
>
> 最近核对：2026-07-23。
>
> 权威来源：[Create chat completion](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)。

## 外部接口关键语义

- 请求路径是 `POST /v1/chat/completions`。
- 请求以 `model` 和 `messages` 为核心；具体可用字段和角色取决于模型与上游实现。
- 非流式响应通常是一个 Chat Completion JSON 对象。
- `stream=true` 时，响应使用 server-sent events，逐步返回 Chat Completion chunk；OpenAI 风格流通常以 `data: [DONE]` 结束。
- 流式 usage 依赖上游支持；OpenAI 兼容实现常通过 `stream_options.include_usage=true` 在终端前提供 usage。

完整字段、工具调用、结构化输出和模型限制必须查阅官方文档及目标上游文档。

## ai-gateway 行为

- 此路径只匹配 `open_ai_chat_completions` 模型规则、渠道组和渠道。
- 网关不完整校验 `messages`、tools 或模型专属字段；完成最小路由校验后交给上游。
- 模型别名只改写顶层 `model`，嵌套对象中的同名字段不变。
- 无变换时，请求 JSON 的空白、键顺序和原始字节保持不变。
- 普通响应状态和 body 默认透传。
- SSE 无变换时按原始字节转发；启用变换时按完整 SSE frame 应用格式专属规则。
- usage 采集识别 `prompt_tokens`、`completion_tokens`、`prompt_tokens_details.cached_tokens`，并兼容部分上游的缓存字段别名。
- 日志把 `[DONE]` 视为 Chat Completions 流的成功终止信号之一。

## 接入检查

接入新的 Chat Completions 兼容上游时，至少验证：

1. base URL 与 `/v1/chat/completions` 拼接正确。
2. 鉴权使用 Bearer 还是自定义 Header。
3. 非流式响应包含预期的 `usage`。
4. SSE `Content-Type` 是 `text/event-stream`，事件以空行分隔。
5. 流式 usage 和终止帧是否符合预期。
6. 上游是否支持客户端要使用的 tools、response format、reasoning 或其他扩展字段。

转发路径变更后，按 [真实上游 smoke test](../development/real-upstream-smoke.md) 验证。
