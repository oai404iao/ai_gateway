# Chat Completions 参考

> 类型：外部参考。
>
> 最近核对：2026-08-05。
>
> 权威来源：[OpenAI Create chat completion](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、
> [DeepSeek Create Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion)、
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode)、
> [DeepSeek Context Caching](https://api-docs.deepseek.com/guides/kv_cache) 与
> [阿里云百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking)。

## 外部接口关键语义

- 请求路径是 `POST /v1/chat/completions`。
- 请求以 `model` 和 `messages` 为核心；具体可用字段和角色取决于模型与上游实现。
- 非流式响应通常是一个 Chat Completion JSON 对象。
- OpenAI usage 使用 `prompt_tokens`、`completion_tokens` 与 `total_tokens`；缓存命中和
  reasoning 细分位于 `prompt_tokens_details.cached_tokens` 与
  `completion_tokens_details.reasoning_tokens`。`completion_tokens` 是包含 reasoning 的输出总量。
- DeepSeek 同样使用 `prompt_tokens` 与 `completion_tokens`，并可额外返回
  `prompt_cache_hit_tokens`、`prompt_cache_miss_tokens` 和
  `completion_tokens_details.reasoning_tokens`。
- 部分 OpenAI-compatible 上游使用额外顶层字段控制思考模式，例如 `thinking` 或
  `enable_thinking`；字段结构和可用模型由对应上游定义。
- `stream=true` 时，响应使用 server-sent events，逐步返回 Chat Completion chunk；OpenAI 风格流通常以 `data: [DONE]` 结束。
- 流式 usage 依赖上游支持。OpenAI 在 `stream_options.include_usage=true` 时于
  `[DONE]` 前发送 `choices: []` 的 usage 汇总 chunk；DeepSeek 兼容响应也可能把 usage
  直接附加在带非空 `finish_reason` 的最终内容 chunk。

完整字段、工具调用、结构化输出和模型限制必须查阅官方文档及目标上游文档。

## ai-gateway 行为

- 此路径只匹配 `open_ai_chat_completions` 模型规则、渠道组和渠道。
- 顶层字段必须进入 `chat_completions.client_body` 白名单；未知顶层字段返回本地 `400`。
- `thinking` 和 `enable_thinking` 已作为第三方兼容扩展列入白名单；网关不解释其值，并在没有
  其他 body 改写时按原始请求字节转发。
- 网关不递归校验 `messages`、tools 或模型专属嵌套结构；顶层检查后仍由目标上游解释。
- 模型别名只改写顶层 `model`，嵌套对象中的同名字段不变。
- 客户端 policy 未删除字段且无变换时，请求 JSON 的空白、键顺序和原始字节保持不变。
- 普通响应状态和 body 默认透传。
- SSE 无变换时按原始字节转发；启用变换时按完整 SSE frame 应用格式专属规则。
- usage 采集把 `prompt_tokens` 记录为输入总量，把 `completion_tokens` 原样记录为输出总量，
  并把 `completion_tokens_details.reasoning_tokens` 作为输出子集单独保留，不从输出总量中扣除。
- 缓存命中优先读取 DeepSeek 的 `prompt_cache_hit_tokens`，否则读取 OpenAI 的
  `prompt_tokens_details.cached_tokens`；`prompt_cache_miss_tokens` 不单独存储，未缓存输入由
  输入总量减缓存命中得到。
- SSE 同时接受 OpenAI 的空 `choices` usage 汇总 chunk 与 DeepSeek 的
  `finish_reason` usage chunk；若两者都出现，以后到达的最终汇总为准。
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
完整顶层字段和 Header 契约见
[`请求字段与 Header 白名单`](request-allowlists.md)。
