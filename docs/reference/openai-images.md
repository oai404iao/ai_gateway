# OpenAI Images API

> 类型：外部参考与网关兼容边界。
>
> 最近核对：2026-08-06（OpenAI 模型与默认值），2026-11-20（[Codex Images 类型](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/images.rs)）。
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

## 请求与结果

OpenAI 的请求参数取决于模型。核对时 GPT Image 返回 base64，默认 `output_format` 为 PNG，不接受 DALL-E 的 `response_format`。网关使用配置的上游模型，不补这些默认值；两种操作均逐块转发非流式 JSON，增量读取顶层 usage。设置 `stream:true` 返回 `400 image_streaming_unsupported`。开始上游尝试后，网关不重试图片请求，以免重复生成和收费。

生成使用普通 JSON 大小限制。编辑使用以下独立的 multipart 规则。

入口必须带合法 multipart boundary，一个非空 `model` 和至少一个 `image`/`image[]`。
字段依 `images_edit.client_body` 分类；未列出的 part 不转发，`moderation=auto` 显式忽略，其他 moderation 值报错。
文本值、文件内容不得出现在日志、错误和审计中。

| 限制 | 当前边界 |
| --- | --- |
| 输入图片 / mask / part | 最多 16 张 / 1 个 / 64 个 |
| 文本字段 | 单项 64 KiB，合计 1 MiB |
| framing | boundary 70 bytes；preamble 8 KiB；单个 part Header block 16 KiB；boundary padding 1 KiB |
| 默认总 body / 单文件 | 64 MiB / 50 MiB，使用独立 `image_edit_*` 配置 |
| 默认内存阈值 | 1 MiB，超出后写受限目录中的匿名临时文件 |

普通渠道在无需改模型或删 part 时回放原始 multipart，否则流式重建。edit 支持 Header 变换；配置请求 JSON 变换则返回 `image_edit_json_transform_unsupported`。图片值不写入日志、错误或审计。上传限额的设置见[配置指南](../user/production-configuration.md#监听器与请求体限制)。

## Codex 插件

固定版本的原生 image tool 使用 `gpt-image-2`；上游操作及字段由安装的插件适配。

| 方面 | 核对的 Codex 适配边界 |
| --- | --- |
| generation | 改为 Codex `/images/generations`，按插件 wire type 过滤字段 |
| edit | 回放流转为 JSON `images[].image_url` data URL，最多五张，不接受 mask |
| 动画 | 支持 GIF 的插件保留 `output_format=gif`；同层可选 `frame_count`（2–16）、`frame_duration_ms`（20–2000）为整数，edit 表单用十进制文本；未提供的值不补全 |
| 结果 | 上游返回的 `data[].generation_id` 随响应透传；网关不会合成 ID |
| 默认值 | `output_format=png` 可在插件层删除；edit `moderation=auto` 在公共入口已删除 |
| Header | 插件注入 OAuth/可选 account/FedRAMP、身份和 image-turn Header |
| 隐私 | 保留合法 session/thread 等关联，补缺失值，installation/workspaces 按插件策略归一化 |

使用[公共请求白名单](request-allowlists.md)判断字段能否进入网关；使用 [Codex 接入参考](codex-oauth-connect.md)区分公共请求与订阅后端协议。
