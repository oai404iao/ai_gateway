# OpenAI Images 转发

> 状态：当前。

Images 接口支持非流式 JSON generation 和 multipart edit。两者都是 `OpenAiImages`，但按操作分别匹配能力、候选与上游目标；`stream: true` 不受支持。

## 本页内容

- [选择操作与路由](#格式与操作分离)
- [跟踪 JSON generation](#json-generation)
- [接收与回放 multipart edit](#multipart-edit)
- [理解 Codex 插件差异](#codex-images)
- [处理大小、磁盘和重试失败](#错误与资源边界)
- [修改后验证](#测试与验收)

## 格式与操作分离

| 公共路径 | 操作 | 请求 | 响应 |
| --- | --- | --- | --- |
| `POST /v1/images/generations` | `images_generation` | JSON | 非流式 JSON |
| `POST /v1/images/edits` | `images_edit` | 带 boundary 的 multipart/form-data | 非流式 JSON |

格式固定为 `OpenAiImages`，选路和日志按操作区分，edit 不能借 generation 或 Responses 候选。导入 Codex 凭证不产生渠道、能力、授权或规则；需要显式配置的关系见[上游身份与能力](upstream-identity-capabilities.md)。

## JSON generation

```text
JSON body within proxy_body_bytes
  -> client top-level allowlist and Fast filtering
  -> Images operation route and model alias
  -> template defaults + capability overrides
  -> connector preparation + final outbound guards
  -> upstream dispatch (no automatic retry)
  -> streamed JSON usage scan + response pass-through
```

`model` 必须是非空字符串，最多 300 字符。`stream: true` 在联系上游前返回
`400 image_streaming_unsupported`。入口使用 `images_generation.client_body` 白名单；没有字段
删除、别名或 body 变换时保留原始请求字节。

`general` 沿用 `/v1/images/generations`、query 和通用 Header/鉴权顺序。上下游独立协商
content coding：上游支持 gzip、RFC 1950 deflate、Brotli、Zstandard 和受限的多层解码，
usage collector 只读取解码后的 JSON 流，再按下游协商重编码，不为完整 base64 图片缓冲响应。
未知或过深 coding 在下游响应头前拒绝；已派发流中发现损坏则终止当前 body，不重试。

普通 Images 解析器增量读取顶层 usage。插件可按实际上游接口声明其他 usage 配置，
不能只从客户端 Images 格式推断；Codex 协议 3 使用通用 Responses usage parser。
计数规则见[usage 规范化](usage-normalization.md)。

## Multipart edit

### 捕获与回放

```text
downstream multipart
  -> Memory(Bytes) until image_edit_memory_bytes
  -> anonymous TempFile above threshold
  -> bounded inspection: model, fields, parts, sizes
  -> exact replay or streamed equivalent rebuild
  -> reqwest Body stream
```

`ReplayableRequestBody` 从内存开始，超过阈值后将已有与后续字节写入配置的 spool。
Unix 目录和文件权限分别收紧为 `0700`、`0600`；只保留匿名句柄，不保留客户端文件名对应路径。
正常结束、错误或取消时随最后一个句柄关闭而释放 inode。

`general` 在没有模型别名和入口字段删除时原样回放；否则沿用原 boundary 流式等价重建。
这不要求完整 multipart、图片或重建 body 同时驻留内存。

### 独立大小限制

| 边界 | 当前值或来源 |
| --- | --- |
| 总 body | `image_edit_body_bytes`，默认 64 MiB |
| 单个 image/mask part | `image_edit_file_bytes`，默认 50 MiB |
| 内存阈值 | `image_edit_memory_bytes`，默认 1 MiB |
| 落盘目录 | `image_edit_spool_directory` |
| part / image / mask 数 | 最多 64 / 16 / 1 |
| 文本字段 | 单项最多 64 KiB，合计最多 1 MiB |
| boundary | 最多 70 bytes |
| preamble / part Header block / boundary padding | 最多 8 KiB / 16 KiB / 1 KiB |

这些 framing 限制防止“body 已落盘但 parser 再次无界分配”，均在派发前执行。
请求须恰好包含一个非空、最多 300 字符的 `model`，以及至少一个 image part。

### 字段与变换

入口按 `images_edit.client_body` 检查顶层字段；未知 part 不转发。
兼容字段 `moderation=auto` 删除，其他 moderation 值拒绝。
非 multipart Content-Type、非法 boundary/framing、非 identity Content-Encoding 和 streaming
均 fail closed。公开接口不接收 JSON/data-URL edit。

multipart edit 不执行请求 JSON Transform。选中能力配置该规则时返回
`400 image_edit_json_transform_unsupported`；Header 与响应 Header Transform 仍生效。
格式中性空模板为各格式生成 no-op，显式 Images 文档不允许 SSE 规则。
无论模板还是能力 override，都不能通过 Header 修改绕过最终出站 guard。

## Codex Images

`connector_kind = codex`、凭证子类型 `codex_oauth`，没有独立的 `codex_images_oauth`。
Codex 是[管理员安装的插件](connector-plugins.md)，provider 白名单、协议 Header、隐私投影和
安全补全由插件维护，宿主不复制 provider 字段表。

### Generation

插件把目标准备为所选接入下的 `/images/generations`，应用 generation body 策略；
仅删除明确等价的 provider 默认字段，无法表达的非默认值拒绝。它不增加 Responses 的
`stream` / `store`，成功响应仍按非流式 JSON 处理。

认证包括 Bearer、存在时的 `ChatGPT-Account-ID`、可选 FedRAMP 和插件配置的
`originator`、版本、User-Agent。有效 `x-codex-image-turn-id` 保留，缺失或不可用时补全。
session/thread/request/window/turn metadata 与 Responses 共用保留/缺失补全规则，
installation/workspaces 仍执行凭证级标识和合成工作区隐私归一化，但不启用 Images affinity。

### Edit

插件通过宿主提供的有界流式适配把 multipart 转成 `/images/edits` 的 JSON：

```text
image parts
  -> incremental base64
  -> images[].image_url = data:<mime>;base64,...
  -> second replayable body
```

最多五张图片；`mask`、第六张图片和无法等价忽略的字段均拒绝。公共 multipart 的 16 张上限
与该 provider 上限相互独立。文本投影保留 `prompt`、`background`、`model`、`n`、`quality`、
`size` 和[兼容的 GIF 控制字段](../reference/openai-images.md#codex-插件)；
`output_format=png` 等默认值按插件策略删除，其他不支持值 fail closed。
使用 generation 相同的认证、image-turn 与隐私规则，不把完整 data URL JSON 放进单个缓冲。

原始 multipart、适配后的 JSON 和图片内容均不得进入 tracing、请求日志、audit 或错误响应。
客户端动作来源是[公共白名单契约](../reference/request-allowlists.json)；provider 动作来源是
独立插件，分工见[白名单维护说明](../reference/request-allowlists.md)。

## 错误与资源边界

| 情况 | 行为 |
| --- | --- |
| Images 上游尝试已开始 | 不自动重试、不切换候选，包括响应头前错误或超时 |
| Codex 凭证在派发前 draining/unavailable/disabled | 排除当前候选，按显式操作路由选择其他合格凭证 |
| 已派发 Codex 请求收到 401 | generation 去重触发后台 refresh，不重放图片请求 |
| 能力未显式设置响应头超时 | 使用 Images 专用系统超时，默认 300 秒 |
| 能力显式设置响应头超时 | 覆盖系统默认；建连与 stream-idle 仍使用通用策略 |
| spool 创建、写入或初始 seek 失败 | `503 image_body_spool_unavailable`，不派发 |
| 派发期间文件读取失败 | 终止当前尝试，计入存储失败，不重试 |

Images 不使用 Session affinity、WebSocket、SSE Transform 或 scheduled probe；测试模型配置
在写入/编译边界拒绝。没有重复生成或重复扣费的可靠证明时，不能借普通 pre-header retry
重新发送 Images。

`GET /console/v1/system/load` 的 `image_body_spool` 提供活跃文件/字节、累计写入、存储失败和
文件系统可用容量；容量低于三个最大 edit body 时还产生 `ai_gateway::image_body_spool` warning。
监控不是磁盘预留，仍须保留写失败路径。

## 测试与验收

| 改动面 | 必须保留的确定性覆盖 |
| --- | --- |
| Generation | 路由与格式隔离、原始字节、认证、JSON usage、大 base64 不整体缓冲 |
| HTTP coding | 四种 coding、多层解码、独立下游协商、identity 回退、失效表示 Header 清理和未知 coding 拒绝 |
| Edit | exact replay、模型别名/字段删除重建、阈值落盘、权限、重复读取、取消与 Drop 清理 |
| 负向输入 | body/file/part/image/mask 限制、Content-Type/Encoding、streaming、JSON Transform |
| Codex | 插件实际库、路径与认证、image-turn、两张图片 data URL、五张上限、mask 拒绝与脱敏 |
| 财务与日志 | operation、当前格式终态的归属、独立计量事实、一次结算、Console 投影 |
| 重试与超时 | 两个可选候选仍只派发一次、Images 专用/显式超时、派发前凭证筛除 |

转发改动须按仓库规则运行 Rust/相关 Console 检查，以及经授权的[真实上游 smoke](real-upstream-smoke.md)。
完整 `REAL_UPSTREAM_IMAGES_*` 配置额外启用付费 generation/edit。
确定性测试验证 multipart 回放、上游只派发一次以及失败后的计量，不依赖真实上游制造这些故障。

## 权威实现位置

| 主题 | 来源 |
| --- | --- |
| 格式与操作 | `src/domain/api_format.rs`、`src/domain/api_operation.rs` |
| 公共路径与代理 | `src/http/mod.rs`、`src/application/proxy.rs` |
| replayable body 与 multipart | `src/application/request_body.rs` |
| usage 与响应适配 | `src/application/usage.rs`、`src/application/response_adapter.rs` |
| Transform 与编译 | `src/transforms/`、`src/runtime_config/mod.rs` |
| Console 形状 | [Console OpenAPI](../openapi/console-v1.yaml) |
| 用户行为与外部兼容 | [运维接口](../user/data-plane.md)、[Images 参考](../reference/openai-images.md) |
