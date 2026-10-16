# 请求字段与 Header 白名单

> 类型：外部兼容参考与网关安全边界。
>
> 状态：当前。
>
> 最近核对：2026-09-08。外部来源版本沿用原记录，未因本文重排更新。
>
> 机器可读权威契约：[request-allowlists.json](request-allowlists.json)。
>
> 权威来源：[openai-node 固定请求类型](https://github.com/openai/openai-node/tree/854892a0580980449ce1ed04aa5e3831d3330383)及文末各源码记录。

如果请求在联系上游前返回未知字段错误，先查本页。这里解释策略如何执行，不复制全部字段表；
新增字段必须修改机器契约及测试，不能只在 Markdown 中声明支持。

## 先判断在哪一层失败

| 例子 | 公共入口 | 后续处理 |
| --- | --- | --- |
| Chat 顶层 thinking / enable_thinking | 允许，不解释值 | 目标上游决定是否支持 |
| Responses 顶层 max_output_tokens | 允许 | 普通上游可保留，Codex 插件可按自己的契约删除 |
| 未声明顶层字段 | 400 request_body_field_unsupported | 不联系上游 |
| edit moderation=auto | 删除后继续 | 不发送该 part |
| edit moderation=low | 400 request_body_field_value_unsupported | 不能把非等价值静默删除 |
| X-Forwarded-For | 显式忽略 | 变换/插件/鉴权之后仍不得重新引入 |
| 未知自定义 Header | 入口忽略 | 普通管理员变换可加未受保护 Header，不等于客户端任意透传 |

“入口允许”不是递归 schema 验证，也不证明模型支持。允许字段内部的 messages/input/tools/metadata
由插件或上游解释；普通 general 不执行 Codex 隐私归一化。

## 目标

宿主拥有公共客户端 Header、顶层 body 和通用出站安全约束。
服务商协议、字段/隐私策略属于插件：
[ai-gateway-connectors](https://github.com/oai404iao/ai-gateway-connectors) 的
`codex/src/request-allowlists.json` 是 Codex 出站规则，**不在本仓库公共 JSON 中**。

这种分层允许公共 Responses 支持的字段在某个插件中被明确拒绝或等价删除，而不是为迁就插件
缩小所有普通上游的能力。插件不能通过自己的字段处理绕过宿主模型、凭证、Fast 或 Header 边界。

## 动作语义

| 动作 | body | Header |
| --- | --- | --- |
| allow | 保留；其他阶段仍可能改写 | 进入后续处理，不保证最终原样出站 |
| ignore | 删除；accepted_values 存在时只接受声明的等价值 | 删除 |
| reject | 本地 400，不派发 | 由对应策略处理 |
| 未列出 | 默认 reject | 默认 ignore |

ignore 只适合遥测、空值/默认值、明确 no-op 或有来源依据的兼容行为，不能用于吞掉会改变生成
语义的未知字段。JSON 字节保真要求所有后续阶段也未修改 body；模型别名、Fast 过滤和插件适配
都可能触发重写。

## 客户端入口策略

### Header

六种数据面操作共享 client_headers，允许的完整名称和前缀只在机器契约维护。
常见类别包括：

- OpenAI 组织/项目、幂等与 HTTP 表示 Header；
- session-id/thread-id/x-client-request-id/x-codex-window-id/x-session-id；
- x-codex-image-turn-id 与 Codex turn metadata、beta、routing、turn-state 控制字段；
- W3C trace/baggage 和官方 SDK x-stainless-*；
- 旧 session_id/thread_id 仅为兼容既有配置，新配置使用连字符形式。

Authorization 用于 Gateway 鉴权，Host/Content-Length 等用于 framing，
Accept-Encoding 由 transport 重新协商：这些名称在入口 allow，不代表透传到上游。
Connection 声明的动态 hop-by-hop 名称暂存至安全清理，防止变换绕过动态保护，最终绝不转发。

Forwarded、Via、常用 X-Forwarded-*、真实 IP、Cloudflare 转发元数据被显式列入 ignore。
它们在入口删除，管理员变换后清理，插件/认证准备后最终 guard 再检查。
模型发现、scheduled probe 等会执行 Header Transform 的内部路径也使用相同 guard。
自定义认证和插件生成 Header 不能与显式 ignore 集冲突。

Session affinity 的 request_header 来源必须在客户端白名单内，否则控制面编译失败。
未知 Header 的默认入口 ignore 与显式 ignore 的出站禁令不同，不能混为一谈。

Content-Encoding 只有 HTTP Responses 接受 identity/zstd；zstd 在 policy 前解码，
压缩和解码后大小都受 proxy_body_bytes 限制。其他 JSON 只接受 identity，multipart edit
拒绝 encoded body。该入口能力与上游是否启用出站压缩是两回事。

### Body

机器接口键为稳定 policy 标识，不应当作 Console 的操作枚举：

| 机器契约键 | 公共入口 | 当前操作名 |
| --- | --- | --- |
| chat_completions | POST /v1/chat/completions | chat_completion |
| responses_http | POST /v1/responses | responses |
| responses_websocket | WS response.create | responses-ws |
| standalone_web_search | POST /v1/alpha/search | web_search |
| images_generation | POST /v1/images/generations | images_generation |
| images_edit | multipart POST /v1/images/edits | images_edit |

client_body.allow/ignore/reject 决定完整顶层分类。未知字段为
request_body_field_unsupported；已知 ignore 字段使用非等价值为
request_body_field_value_unsupported。multipart 使用相同字段分类，额外 framing、文本与文件限制
见[Images](openai-images.md)。

service_tier 在支持它的公共入口仍为 allow。用户组 filter_fast_mode 在白名单通过后静默删除它，
不改变机器字段分类；变换、插件、日志、affinity 和请求倍率只观察过滤后的请求，不能重新引入。

edit 的 output_format 在入口保留，由选中上游/插件决定后续动作；
moderation=auto 的公共 ignore 与 Codex 插件删除 output_format=png 是不同层的规则。

## 执行顺序

```text
API Key / framing / body size / supported request decoding
  -> client top-level body allowlist
  -> user-group Fast filtering
  -> client Header allowlist
  -> operation/model routing and top-level model alias
  -> template defaults, then channel overrides
  -> plugin protocol/body/Header/privacy preparation
  -> shared hop-by-hop and explicit-ignore cleanup
  -> upstream authentication and credential-binding validation
  -> final explicit-ignore guard
  -> transport framing / final HTTP compression / handshake
```

body 变化会清掉原始 Content-MD5、digest、ETag 等失效表示元数据，避免发到上游。
multipart 用 replayable 流重建删除/替换 part，不把完整图片读回内存。
插件必须保持选定模型、凭证目标范围和 Fast 过滤结果；失败应终止派发。

## 维护流程

**公共字段/Header 变更**：

1. 查官方请求类型或供应商原始来源，记录实际核对日期和 commit。
2. 先改本仓库 request-allowlists.json，再同步说明与入口/最终 guard 测试。
3. 为每个字段明确 allow/ignore/reject；ignore 值写清等价依据。
4. 反向代理/CDN Header 加入统一 ignore 集，不在代理代码里另建名单。
5. 运行相应 Rust、文档检查；转发实现变更按[真实 smoke 流程](../development/real-upstream-smoke.md)处理授权。

**服务商字段/隐私变更**：改插件契约和插件测试，同时保留 gateway-boundary 确定性回归；
不把新 provider 规则投影到宿主 system settings 或公共 JSON。

宿主单元测试检查六接口完整性、分类互斥、未知默认动作、Header 合法性/排序/重复项，以及来源
commit 和日期格式。这些结构检查不能替代对外部语义的实际核验。

## 外部类型核对来源

下列为原核对记录；Responses 和独立 Images/Search 的源码版本分别保存。
> 权威来源：
> [`openai/openai-node@854892a`](https://github.com/openai/openai-node/tree/854892a0580980449ce1ed04aa5e3831d3330383) 的
> [Chat Completions](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/chat/completions/completions.ts)、
> [Responses](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/responses/responses.ts) 与
> [Images](https://github.com/openai/openai-node/blob/854892a0580980449ce1ed04aa5e3831d3330383/src/resources/images.ts) 请求类型；
> [`openai/codex@fde2156`](https://github.com/openai/codex/tree/fde2156057c38c0227ce94c8514d04c7498df60d) 的
> [Responses client and Header construction](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/core/src/client.rs)、
> [default HTTP client identity](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/login/src/auth/default_client.rs)、
> [Responses SSE turn state](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/sse/responses.rs)、
> [Responses WebSocket endpoint](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/endpoint/responses_websocket.rs)、
> [Images wire type](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/images.rs) 与
> [Search wire type](https://github.com/openai/codex/blob/fde2156057c38c0227ce94c8514d04c7498df60d/codex-rs/codex-api/src/search.rs)；
> 请求压缩实现还与本地研究 checkout
> `openai/codex@eb9dceba1a2e658142a456c5898836774835616b` 的
> `codex-rs/http-client/src/request.rs` 交叉核对；
> [DeepSeek Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode) 与
> [阿里云百炼深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking) 的
> Chat Completions 兼容扩展。

独立 Images/Search 另以
[`openai/codex@d648947`](https://github.com/openai/codex/tree/d6489472f3c15e87d2d7763a5fde033545c530f8)
的 [Images Header 构造](https://github.com/openai/codex/blob/d6489472f3c15e87d2d7763a5fde033545c530f8/codex-rs/ext/image-generation/src/backend.rs)、
[Search 请求构造](https://github.com/openai/codex/blob/d6489472f3c15e87d2d7763a5fde033545c530f8/codex-rs/ext/web-search/src/tool.rs)
与 [Search 可选上下文](https://github.com/openai/codex/blob/d6489472f3c15e87d2d7763a5fde033545c530f8/codex-rs/ext/web-search/src/history.rs)
核对，记录为 `sources.codex_standalone_commit`；Responses 的核对版本不因此更新。
