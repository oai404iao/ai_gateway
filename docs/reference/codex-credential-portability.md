# Codex 凭证导入格式兼容性

> 类型：外部格式参考与当前导入边界。
>
> 最近核对：2026-08-03。外部格式按下列固定 commit 理解，未重新核验未来版本。
>
> 权威来源：CLIProxyAPI 的 [Token 定义](https://github.com/router-for-me/CLIProxyAPI/blob/928478e4b91533cec05a763bfac3edad9c3e76cf/internal/auth/codex/token.go)、
> [文件元数据](https://github.com/router-for-me/CLIProxyAPI/blob/928478e4b91533cec05a763bfac3edad9c3e76cf/internal/watcher/synthesizer/file.go)；
> Sub2API 的 [导出结构](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/handler/admin/account_data.go)、
> [导入与身份 fallback](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/handler/admin/account_codex_import.go)、
> [可选 account Header](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/service/openai_chatgpt_headers.go)。

## 先导入草稿，再绑定渠道

在 Console“上游凭证 → Codex”的高级导入页粘贴 JSON 或选择文件。所有解析先在浏览器中完成，
进入可检查、可修改的草稿，不会仅因打开文件就创建渠道或发送数据面请求。

1. 检查 label、enable、account/user ID、Token、quota threshold 和维护代理。
2. 把外部代理映射到现有代理，或检查后显式创建；选择直连会保留未映射警告。
3. 逐条提交；服务端验证 Token、身份和 models 后保存凭证。
4. 再显式创建接入、逻辑渠道和操作能力，绑定凭证，配置路由与授权。

文件最多 20 个、每个 5 MiB。文件和导出 Bundle 含明文 OAuth/代理凭据，不要提交到 Git、
贴入工单、写日志或存入浏览器持久化状态。逐步操作见[凭证管理](../user/upstream-credentials.md)。

## 外部格式关键语义

| 来源 | 识别形状 | 关键差异 |
| --- | --- | --- |
| CLIProxyAPI | `type:"codex"` 单对象或对象数组 | 顶层 Token、account_id、email，可带 proxy_url |
| Sub2API | `type:"sub2api-data"`、version 1、proxies/accounts | openai/oauth 账户，credentials 内 Token；proxy_key 关联代理 |
| Sub2API 响应封装 | 上述 payload 包在 data 中 | 兼容常见 code/data envelope |
| 通用 Token 对象 | 单个可识别 Token 对象 | 不要求伪造某外部产品的 type |

固定 Sub2API 版本不强制 chatgpt_account_id，允许 user_id/sub 等身份 fallback；
Header helper 只在 account ID 非空时发送 chatgpt-account-id。
缺 account ID 不是 Free plan 专属，也不能由迁移工具补造 workspace ID。
这些是版本观察，不承诺对方未来 schema 不变。

## ai-gateway 兼容行为

### 原生 Bundle 与旧字段

原生类型是 `ai-gateway-codex-credentials`；当前导出 **version 3**，导入器仍识别旧版及
上述外部形状。导出可带维护代理定义/认证、身份、enable 和 quota threshold，
不包含渠道组、逻辑渠道归属或 routing weight。

旧 Bundle 或外部对象中的 `weight` 被忽略并在草稿警告，不提交服务端。
新导入凭证不会恢复旧 `all`/`selected` assignment，也不会自动加进模型规则；
路由候选的渠道、wire model 和正权重由相应操作规则显式拥有。

### 身份、去重与验证

`access_token`、`refresh_token` 必须有值，`id_token` 可缺；缺少时从 access token 提取身份。
account ID 可缺，但须取得 user ID，解析兼容 auth namespace 中 chatgpt_user_id/user_id
及 JWT 顶层 sub。显式身份与 Token claim 冲突会被拒绝。

浏览器优先按 `(account ID, user ID)`，其次 `(account ID, email)`，accountless 时按
personal user ID，最后才用相同 Token 检测重复草稿。同一 Business workspace 的不同成员
不能仅因 account ID 一样而去重。

服务端仍独立验证并匹配账号；同一可识别账号更新原凭证，多条历史记录含糊匹配时拒绝静默合并。
重新导入不复制或修改现有渠道、能力、目录和路由。models/quota/数据面仅在 account ID
存在时发送 ChatGPT-Account-ID。JWT 解码本身不证明 Token 有效。

## 差异与限制

- 只导入可识别的 OpenAI Codex OAuth 账号，其他平台/账户类型跳过。
- Sub2API concurrency、priority、rate multiplier、备用代理和过期调度不映射为网关配置。
- CLIProxyAPI last refresh、expired 和未知扩展不直接成为控制面字段。
- 代理必须显式检查映射，不通过导入绕过目标范围或凭证校验。
- 网关只导出自身 Bundle，不生成 CLIProxyAPI/Sub2API 文件，也不承诺无损往返外部产品。
- 维护代理和数据面接入代理不同；导入代理不自动替换已有逻辑渠道的转发网络策略。

## 维护检查项

修改格式识别时，对比固定来源并记录实际重新核对的日期，同时覆盖：

1. CLIProxyAPI 单对象/数组、缺 id_token、proxy_url。
2. Sub2API payload/envelope、accountless 身份、proxy_key 和内嵌代理认证。
3. 当前原生 Bundle 往返、旧版兼容、weight 警告与不恢复旧路由赋值。
4. 草稿去重、Business 多成员、服务端含糊匹配拒绝和 claim 冲突。
5. 文件限制、部分失败重试、禁用/删除代理、未映射代理确认与敏感字段脱敏。

解析器与页面测试位于 `web/console/src/features/admin/providers/codex-oauth/`；
OAuth 生命周期与存储约束见[Codex 开发文档](../development/codex-oauth-connector.md)。
