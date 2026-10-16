# OpenAI Images API

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-06。外部模型与默认值沿用该次观察，不代表最新 catalog。
>
> 权威来源：[Images API Reference](https://developers.openai.com/api/reference/resources/images)、
> [Image generation guide](https://developers.openai.com/api/docs/guides/image-generation)、
> [Images streaming](https://developers.openai.com/api/docs/guides/image-generation#streaming)。

## 选择操作

| 任务 | 公共入口 | 当前支持 |
| --- | --- | --- |
| 文字生图 | `POST /v1/images/generations` | JSON 请求、非流式 JSON 响应 |
| 上传图片编辑 | `POST /v1/images/edits` | multipart 请求、非流式 JSON 响应 |
| JSON/data URL edit | — | 不支持公共入口 |
| 图片 SSE / WebSocket | — | 不支持 |

两种操作都属于 `open_ai_images`，但 `images_generation`、`images_edit` 能力和模型操作路由
分别配置。模型名来自管理员设置，不在普通 `general` 连接器中硬编码。

## 最小请求

以下 `GATEWAY_BASE_URL` 不含 `/v1`，`your-image-model` 是客户端模型名。
执行会生成/编辑图片并产生费用，不是免费连通性检查。

```bash
curl "$GATEWAY_BASE_URL/v1/images/generations" \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"your-image-model","prompt":"A red fox in the snow."}'
```

编辑使用本地图片，交给 curl 生成 multipart boundary，不手写 `Content-Type`：

```bash
curl "$GATEWAY_BASE_URL/v1/images/edits" \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -F model=your-image-model \
  -F 'prompt=Make the background snowy.' \
  -F 'image=@input.png;type=image/png'
```

输出字段、编码、尺寸和质量仍取决于目标上游。示例不强制 DALL-E 或 GPT Image 专属参数。

## 外部接口关键语义

核对时 OpenAI generation 与 edit 使用独立路径，edit 常用 `image`/`image[]` 及可选 `mask`
上传。响应 JSON 可包含大体积 base64 或上游定义的图片结果，部分模型返回顶层 usage。
核对时 GPT Image 直接返回 base64，不接受 DALL-E 专用 `response_format`，
`output_format` 默认 PNG；这些是外部模型语义，不是网关给所有模型补的默认值。

官方文档还描述 generation streaming，但当前网关没有实现；“官方支持”与“此网关可用”必须分开。

## ai-gateway 当前兼容行为

### generation

- 只接受 JSON，通过 Gateway Key、准入、公共白名单和独立操作路由；未知顶层字段本地拒绝。
- 提取 `model`、可选 `stream`；没有策略删除、Fast 过滤、别名或变换/适配时保留原始字节。
- 请求 JSON 使用全局 `request_limits.proxy_body_bytes`，不因图片响应较大提高该上限。
- 响应逐块转发；从解码后 JSON 增量采集已支持的顶层 usage，不为 base64 全量缓冲。
- 日志为 Images 格式、`images_generation` 操作、`non_stream` 协议。

### edit

入口必须带合法 multipart boundary，一个非空 `model` 和至少一个 `image`/`image[]`。
字段依 `images_edit.client_body` 分类；`moderation=auto` 显式忽略，其他 moderation 值报错。
文本值、文件内容不得出现在日志、错误和审计中。

| 限制 | 当前边界 |
| --- | --- |
| 输入图片 / mask / part | 最多 16 张 / 1 个 / 64 个 |
| 文本字段 | 单项 64 KiB，合计 1 MiB |
| framing | boundary 70 bytes；preamble 8 KiB；单个 part Header block 16 KiB；boundary padding 1 KiB |
| 默认总 body / 单文件 | 64 MiB / 50 MiB，使用独立 `image_edit_*` 配置 |
| 默认内存阈值 | 1 MiB，超出后写受限目录中的匿名临时文件 |

body 通过 `ReplayableRequestBody::{Memory, TempFile}` 回放，不持久保留命名上传文件。
普通连接器无 model/忽略字段改动时回放原始 bytes；需要修改时流式等价重建，不把图片全读回内存。
日志操作为 `images_edit`，不会借用 generation 路由。

### 响应编码

上下游独立协商 HTTP content coding：不直接转发客户端 `Accept-Encoding`；
上游支持 gzip、deflate、Brotli、Zstandard 的流式解码，下游按客户端能力重编码。
已知小于 1 KiB 的响应保持 identity；未知长度不为阈值判断缓冲完整 JSON。
具体错误与最多编码层数见[兼容性总览](openai-compatibility.md#响应兼容策略)。

## 差异与限制

### 所有连接器

- generation/edit 的 `stream:true` 均本地返回 `400 image_streaming_unsupported`，不派发。
- 一次上游尝试开始后不自动重试或跨候选故障转移，避免重复生成/计费。
- 不使用 Images Session affinity、SSE 事件变换、WS 或付费定时测试模型。
- multipart edit 请求 JSON Transform 不可用，选中时返回
  `image_edit_json_transform_unsupported`；Header/响应 Header 变换仍可使用。
- generation 与 edit 必须各自配置能力、模型目录、候选和授权；导入凭证不代建路由。

### Codex 插件

Codex 是管理员安装启用的外部插件，不是内置 Images fallback。其核对版本的原生 image tool
使用 `gpt-image-2`，不能据此推断普通上游模型名或未来 Codex catalog。

| 方面 | 核对的 Codex 适配边界 |
| --- | --- |
| generation | 改为 Codex `/images/generations`，按插件 wire type 过滤字段 |
| edit | 回放流转为 JSON `images[].image_url` data URL，最多五张，不接受 mask |
| 默认值 | `output_format=png` 可在插件层删除；edit `moderation=auto` 在公共入口已删除 |
| Header | 插件注入 OAuth/可选 account/FedRAMP、身份和 image-turn Header |
| 隐私 | 保留合法 session/thread 等关联，补缺失值，installation/workspaces 按插件策略归一化 |

上述 Header 关联不启用 Images affinity。凭证可由显式绑定的逻辑渠道复用，维护 Token/quota
不代表共享操作路由或自动授权。适配失败在派发前终止；具体字段动作、合成工作区和身份设置
由插件契约及其不可变 generation 管理，不在宿主公共 JSON 契约定义。

更多来源见[Codex 接入](codex-oauth-connect.md)，公共字段见
[请求白名单](request-allowlists.md)。

## 维护检查项

1. 重新核对官方 generation/edit/streaming 和目标模型字段，再更新外部核对日期。
2. 同步公共机器契约、路由、操作能力、日志和 usage；插件字段在插件仓库修改。
3. 保持 multipart framing、匿名 spool、磁盘容量及流式 base64 的有界资源测试。
4. 验证非重试边界，避免以自动恢复为名重复付费图片操作。
5. 保留确定性 proxy integration coverage；可选真实图片 smoke 不能替代它。

实现设计见[Images 开发文档](../development/openai-images.md)，付费测试条件见
[真实上游 smoke](../development/real-upstream-smoke.md)。
