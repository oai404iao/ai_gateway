# Transform DSL

> 状态：当前。实现与校验以 `src/transforms/` 为准。

`config_templates.document` 与 `channels.override_document` 使用同一份受限
JSON 转换 DSL。空对象 `{}` 是显式 no-op。非空文档必须声明 API 格式，并且
格式必须与所绑定渠道一致。

DSL 内部顺序固定：

```text
模板默认规则 → 渠道覆写规则
```

完整请求顺序为：客户端白名单 → 上述 DSL → Codex body 白名单、隐私归一化与安全补全（如适用）→
Header 清理 → Codex Header 白名单、隐私归一化与安全补全（如适用）→ 上游鉴权注入。因此
Transform 可以添加普通 upstream 字段，但不能绕过 Codex provider policy 或恢复客户端原始
installation/workspace 指纹。

所有请求体规则都不能修改 `model` 或 `stream`；请求和响应 Header 均有受保护
名称。请求侧 `content-encoding` 与 `accept-encoding` 由网关拥有，不能通过 DSL 修改；
响应体规则只作用于受支持的 SSE JSON 事件。

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
