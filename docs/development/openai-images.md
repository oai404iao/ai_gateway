# OpenAI Images 转发

> 状态：部分实现。非流式 JSON generation、multipart edit、Codex 插件适配和 request-body
> spool 已实现；Images streaming 仍未实现。末节保留早期分阶段设计的迁移背景。

Images 使用独立的 `open_ai_images` 格式权限和操作路由，复用网关鉴权、健康、Transform、
日志与计费，不借用 Responses 路由。大图片输入通过可回放 body 落盘，而不是扩大所有 JSON
请求的内存上限。

## 本页内容

- [选择操作与路由](#格式与操作分离)
- [跟踪 JSON generation](#json-generation)
- [接收与回放 multipart edit](#multipart-edit)
- [理解 Codex 插件差异](#codex-images)
- [处理大小、磁盘和重试失败](#错误与资源边界)
- [修改后验证](#测试与验收)
- [查阅旧迁移背景](#历史迁移背景)

## 格式与操作分离

| 公共路径 | 操作 | 请求 | 响应 |
| --- | --- | --- | --- |
| `POST /v1/images/generations` | `images_generation` | JSON | 非流式 JSON |
| `POST /v1/images/edits` | `images_edit` | 带 boundary 的 multipart/form-data | 非流式 JSON |

两者的 `ApiFormat` 都是 `OpenAiImages`，`ApiOperation` 分别为 `ImagesGeneration` 和
`ImagesEdit`。日志同时保留格式和操作；operation 规则、candidate 和能力必须匹配，不能在缺少
edit 路由时借用 generation，也不能回退到 Responses。

当前资源关系是：

```text
upstream access + upstream credential
  -> logical channel
  -> explicit images_generation / images_edit capabilities
  -> operation rule: (capability_id, upstream_model, weight) candidates
```

凭证可以被兼容的多个逻辑渠道复用；格式隔离发生在操作能力和路由，而不是要求一个旧式 Channel
同时投影出 Responses/Images 身份。渠道组提供组织和选项上下文，不持有路由权重。
导入 Codex 凭证不创建接入、渠道、能力、授权或路由。

管理员须显式创建支持的能力、配置模型目录和计价模型下的对应操作规则，再授予逻辑渠道访问。
恢复凭证不会打开已关闭的 Images 能力，也不自动把 Images 格式加入现有 Key/Policy。
完整拓扑见[上游身份与能力](upstream-identity-capabilities.md)。

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

入口按 `images_edit.client_body` 检查顶层字段；未知字段返回
`request_body_field_unsupported`。兼容字段 `moderation=auto` 删除，其他 moderation 值拒绝。
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
`size`，例如 `output_format=png` 的等价值按插件策略删除，其他不支持值 fail closed。
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

## 后续：Streaming

启用 streaming 前必须另行设计事件类型、终态/错误、partial image 大小与背压、专用 Transform
策略、usage/取消/断开后的财务终态及重复请求风险。在此之前 `stream: true` 必须拒绝，
不能把转发普通 SSE 的能力视为 Images streaming 已实现。

## 测试与验收

| 改动面 | 必须保留的确定性覆盖 |
| --- | --- |
| Generation | 路由与格式隔离、原始字节、认证、JSON usage、大 base64 不整体缓冲 |
| HTTP coding | 四种 coding、多层解码、独立下游协商、identity 回退、失效表示 Header 清理和未知 coding 拒绝 |
| Edit | exact replay、模型别名/字段删除重建、阈值落盘、权限、重复读取、取消与 Drop 清理 |
| 负向输入 | body/file/part/image/mask 限制、Content-Type/Encoding、streaming、JSON Transform |
| Codex | 插件实际库、路径与认证、image-turn、两张图片 data URL、五张上限、mask 拒绝与脱敏 |
| 财务与日志 | operation、journal v2/v3 backlog 与后续版本兼容、独立计量事实、一次结算、Console 投影 |
| 重试与超时 | 两个可选候选仍只派发一次、Images 专用/显式超时、派发前凭证筛除 |

转发改动须按仓库规则运行 Rust/相关 Console 检查，以及经授权的[真实上游 smoke](real-upstream-smoke.md)。
完整 `REAL_UPSTREAM_IMAGES_*` 额外启用付费 generation/edit；无论是否配置真实 Images，
确定性插件/代理测试不能被替代。本文的设计记录不表示本次文档修改重新运行了这些验收。

## 历史迁移背景

最初的 PR 1 实现普通 generation，PR 2 增加 Codex generation，PR 3 增加 multipart edit 和
spool。`0034` 增加 Images format，`0035` 增加日志 operation 与旧写入兼容触发器，journal v4
开始保存操作并可读取 v2/v3 backlog。该触发器解释当时升级兼容，不代表当前版本可与任意旧版混跑。

`0036_codex_images_projection.sql` 当时通过 `connector_pools` 和
`codex_oauth_credential_channels` 共享凭证，保留旧 Responses group/channel/credential ID，
创建停用的 Images 配对组及 managed channel。当时 projection 同步 label、proxy、计费倍率和
timeout，但保持模型、健康和授权隔离；Images 固定模型目录使用 `gpt-image-2`，不启用状态统计。
这一历史迁移要求协调停机，不自动授权或创建 Images 路由。

后续[身份与能力联合切换](upstream-identity-capabilities.md)已退役该 projection 拓扑，
改为独立凭证、逻辑渠道和显式操作能力；不能再依赖创建凭证自动生成两个 Channel、
默认固定 Images 模型或删除凭证顺带 tombstone 两个 projection。
当前凭证删除先解除活动渠道引用，清除 Token 并保留凭证墓碑，金融历史不删除。

## 权威实现位置

| 主题 | 来源 |
| --- | --- |
| 格式与操作 | `src/domain/api_format.rs`、`src/domain/api_operation.rs` |
| 公共路径与代理 | `src/http/mod.rs`、`src/application/proxy.rs` |
| replayable body 与 multipart | `src/application/request_body.rs` |
| usage 与响应适配 | `src/application/usage.rs`、`src/application/response_adapter.rs` |
| Transform 与编译 | `src/transforms/`、`src/runtime_config/mod.rs` |
| Console 形状 | [Console OpenAPI](../openapi/console-v1.yaml) |
| 用户行为与外部兼容 | [运维接口](../user/operations.md)、[Images 参考](../reference/openai-images.md) |
