# 调用公共数据面

> 状态：当前。

将客户端的 Base URL 指向 Gateway `/v1`，用 Gateway API Key 鉴权。先查询当前 Key 可达的模型，再调用目标操作；第一次创建路由与 Key 见[快速开始](quickstart.md)。

```bash
curl --fail-with-body http://127.0.0.1:3000/v1/models \
  -H "Authorization: Bearer $GATEWAY_API_KEY"
```

## 选择接口

`GET /health` 无需认证，返回 `204` 仅表示进程存活。`GET /v1/models` 需要 `proxy` 和 `models.read` 权限。列表中出现模型不代表六种操作都可用；每种操作都需要自己的[能力、路由与渠道授权](routing.md)。

| 操作 | 调用方式 | 传输 |
| --- | --- | --- |
| Chat Completions | `POST /v1/chat/completions` | JSON 或 SSE |
| Responses | `POST /v1/responses` | JSON 或 SSE；[Codex](codex.md) 仅 SSE |
| Responses WebSocket | `GET /v1/responses` + Upgrade | 顺序发送 [`response.create`](websocket.md) |
| 独立搜索 | `POST /v1/alpha/search` | 非流式 JSON，Codex 扩展 |
| 生成图片 | `POST /v1/images/generations` | 非流式 JSON |
| 编辑图片 | `POST /v1/images/edits` | multipart 输入，非流式 JSON 输出 |

## 请求字段与压缩

网关忽略未列出的客户端 Header，删除未列出的顶层 JSON 字段和 multipart part；已列字段可能有明确的值限制。允许字段的嵌套结构仍由对应操作或上游解释。具体动作见[公共请求白名单](../reference/request-allowlists.md)。上游认证来自渠道凭证，不是客户端 `Authorization`。

只有 `POST /v1/responses` 接受 `Content-Encoding: zstd` 的客户端 JSON。压缩输入和解压后的 JSON 均受 `request_limits.proxy_body_bytes` 限制。其他 JSON 请求使用 identity，图片编辑使用未编码的 multipart。

## 转发、响应与重试

认证后先检查 RPM、并发与已结算软额度，再读取请求体。无需改模型、删除字段或执行变换时，保留请求原始字节。

上游和客户端分别协商响应压缩。网关可以流式解码上游 `gzip, deflate, br, zstd`，再按客户端支持的编码返回非 SSE 响应。已知小于 `1 KiB` 的响应不压缩；SSE 下游与 Range 请求上游均使用 identity。不支持的上游编码返回 `502 upstream_content_encoding_unsupported`，读取中发现压缩流损坏则终止响应并记录 `upstream_body_error`。

普通 HTTP 请求在连接失败或等待响应头超时后可切换到未尝试的候选。也可显式配置按特定 4xx/5xx 状态码重试，但上游可能再次执行并收费。响应头或内容已发给客户端后不再重试。**图片、已派发的 Codex 请求、已发送的 WebSocket 消息均不自动重试**。策略见[故障转移设置](routing.md#管理故障转移自动禁用与定时测试)。

## 生成或编辑图片

generation JSON 需带 `model`，使用普通 JSON 大小限制。edit 需带 `model`、一个或多个 `image`/`image[]`，可带一个 `mask`；不接受客户端 JSON/data URL 编辑。图片请求设置 `stream: true` 会在派发前返回 `400 image_streaming_unsupported`。

通用 edit 最多 64 个 part、16 张输入图和一个 mask；普通文本字段单项 `64 KiB`、合计 `1 MiB`。总请求、单个 image/mask、内存与临时磁盘分别受[图片上传配置](production-configuration.md#监听器与请求体限制)约束。Codex edit 的限制更严，见[插件兼容范围](../reference/openai-images.md#codex-插件)。

无需改模型或删除 part 时通用渠道收到原始 multipart，否则网关流式重建。edit 支持 Header 变换，但若所选能力配置请求 JSON 变换则返回 `400 image_edit_json_transform_unsupported`。图片能力不参加定时测试或 Session 亲和。

## 使用独立搜索

`/v1/alpha/search` 有独立的顶层字段白名单和非流式响应头超时。`output`、`encrypted_output` 和 `results` 按上游结果转发。可以改模型别名或 Header，不能配置请求 JSON 变换，也不会添加 Responses 的 `stream` 或 `store` 字段。

其他接口的支持范围及网关错误码见[协议兼容性参考](../reference/openai-compatibility.md)。
