# Transform DSL

> 状态：当前。实现与校验以 `src/transforms/` 为准。

Transform DSL 在编译配置时生成有界规则，用于请求 JSON、请求/响应 Header 和受支持的流事件。
它不是脚本运行时，不读取凭据、环境或网络。模板 `config_templates.document` 与操作能力的
`override_document` 使用同一语法；旧 `channels.override_document` 已随拓扑切换退役。

最小配置是 `{}`，表示显式 no-op。需要规则时声明版本和 API 格式，例如：

```json
{
  "version": 2,
  "api_format": "open_ai_chat_completions",
  "request_json": [
    { "op": "add", "path": "/metadata/gateway", "value": "ai-gateway" }
  ]
}
```

该例要求 `/metadata` 父路径已存在；不存在时整个 body 转换失败，不能据 `add` 推断会自动创建
任意祖先对象。非空文档的格式必须与所绑定操作能力一致。

## 本页内容

- [选择版本和操作](#版本)
- [引用当前值](#当前目标值引用)
- [按目标条件执行](#条件执行)
- [检查请求顺序和格式限制](#执行顺序与边界)
- [了解不支持的能力](#仍明确不支持的高级能力)

## 执行顺序与边界

DSL 内部顺序固定：

```text
模板默认规则 → 渠道覆写规则
```

完整请求顺序为：客户端 body 白名单 → 用户组 Fast 过滤 → 客户端 Header 白名单 →
模板默认值 → 能力覆写 → 插件协议/body/Header/隐私准备 → 通用 hop-by-hop/显式 ignore 清理
→ 上游鉴权 → 最终显式 ignore guard → 派发。具体插件调用可分阶段执行，但不能绕过这些边界。
Transform 可添加普通 upstream 字段，不能绕过插件 provider policy、恢复被过滤的客户端本地
installation/workspace 指纹，或重新引入被公共 Header policy 禁止的转发元数据。

所有请求体规则都不能修改 `model` 或 `stream`；请求和响应 Header 均有受保护
名称。请求侧 `content-encoding` 与 `accept-encoding` 由网关拥有，不能通过 DSL 修改；
响应体规则只作用于受支持的 SSE JSON 事件。

| 操作 | 请求 JSON | Header | 响应事件 |
| --- | --- | --- | --- |
| Chat / HTTP Responses | 支持受限规则 | 请求与响应均受保护名称限制 | 受支持的同格式 SSE selector |
| Responses WebSocket | 每次 create 应用对应规则 | 握手与最终协议 Header 仍受 guard | 复用 Responses selector，不含 SSE envelope |
| Standalone search | 禁止 | 支持 | 不使用 SSE 规则 |
| Images generation | 支持 | 支持 | 禁止 SSE 规则 |
| Multipart Images edit | 禁止，配置后 fail closed | 支持 | 禁止 SSE 规则 |

规则在控制面编译，应用时仍执行 Header 安全检查。请求转换原子失败，不发送部分修改的 body；
已经开始的响应流遇到事件转换错误只终止当前流，不能切换渠道或重放。

## 版本

### 版本 1

版本 1 保持原有 RFC 6902 子集：

```json
{
  "version": 1,
  "api_format": "open_ai_chat_completions",
  "request_json": [
    { "op": "add", "path": "/metadata/gateway", "value": "ai-gateway" },
    { "op": "replace", "path": "/temperature", "value": 0.2 },
    { "op": "remove", "path": "/deprecated_field" }
  ]
}
```

其中标准 `add` 可以按 RFC 6902 使用 `/array/-` 追加到数组，或使用
`/array/0` 插入到首位。版本 2 提供更直观且可视化支持的数组操作。

### 版本 2

版本 2 保留 `add`、`replace`、`remove`，并增加：

| 操作 | 作用 |
| --- | --- |
| `array_append` | 向现有数组末尾添加一个值。 |
| `array_prepend` | 向现有数组开头添加一个值。 |
| `array_insert` | 在现有数组的 `index` 位置插入一个值。 |
| `array_remove` | 删除现有数组的 `index` 位置。 |
| `merge` | 将对象值浅合并到现有对象目标；同名字段由新值覆盖。 |

所有数组操作的 `path` 指向数组本身；`array_insert` 和 `array_remove`
必须指定非负整数 `index`。数组或对象目标不存在、类型不符、索引越界时，整份
请求体转换失败，原始请求不会被部分改写。

```json
{
  "version": 2,
  "api_format": "open_ai_chat_completions",
  "request_json": [
    {
      "op": "array_prepend",
      "path": "/messages",
      "value": { "role": "system", "content": "Follow gateway policy." },
      "when": { "type": "array" }
    },
    {
      "op": "merge",
      "path": "/metadata",
      "value": { "gateway": "ai-gateway" },
      "when": { "type": "object" }
    }
  ]
}
```

## 当前目标值引用

版本 2 的 `value` 可递归使用下列精确标记：

```json
{ "$ref": "current" }
```

它会读取**当前操作的 `path` 在操作前的值**。不能读取其他 JSON Pointer、
请求头、模型、渠道、环境变量或密钥。

```json
{
  "op": "replace",
  "path": "/metadata",
  "value": {
    "original": { "$ref": "current" },
    "gateway": "ai-gateway"
  },
  "when": { "type": "object" }
}
```

字符串模板使用：

```json
{ "$template": "gateway-{{value}}" }
```

`{{value}}` 会将当前目标值渲染为文本；字符串保持原样，其他 JSON 值使用紧凑
JSON 表示。若需要字面量对象恰好为标记形状，可使用：

```json
{ "$literal": { "$ref": "current" } }
```

## 条件执行

版本 2 可选 `when` 只检查当前操作的目标路径。一次只能声明一个谓词：

```json
{ "when": { "exists": true } }
{ "when": { "exists": false } }
{ "when": { "type": "array" } }
{ "when": { "equals": ["expected", "value"] } }
```

条件不满足时，该操作是 no-op；条件满足后执行发生错误时，整份请求体转换仍是
原子的，不会保留此前操作的部分结果。

`array_*`、`merge`、`replace` 与 `remove` 都要求目标本身存在，因此不能与
`{ "when": { "exists": false } }` 组合。仅 `add` 可以在目标不存在时执行（其
父路径仍必须存在）。

## 仍明确不支持的高级能力

为避免脚本执行、隐式数据泄露和不可预测的资源消耗，当前不支持：

- 任意 JavaScript、Shell、WASM 或正则脚本；
- 跨路径复制、读取请求头、路由上下文、环境变量、凭据或任意外部变量；
- 深层对象合并、数组过滤/映射/排序、基于表达式的循环；
- 任意 HTTP 调用、网络超时覆写或动态鉴权。

后续若需要扩展，应新增显式 DSL 版本和受限操作，不应改变已有版本的语义。

## 验证与实现入口

`src/transforms/mod.rs` 包含编译器、Header guard、JSON 应用及 SSE/WS selector 回归。
修改语法时同时验证未知字段、跨格式、受保护路径、缺失目标、索引越界、模板组合和错误原子性。
响应适配插件是[另一条显式 opt-in 边界](connector-response-adapters.md)，不能把其状态或 usage
语义写入 DSL。转发行为变更还须遵守[真实上游验证要求](real-upstream-smoke.md)。
