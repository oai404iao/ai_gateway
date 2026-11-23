# Codex 凭证导入格式

> 类型：外部格式参考与当前导入边界。
>
> 最近核对：2026-08-03。
>
> 权威来源：CLIProxyAPI 的 [Token 定义](https://github.com/router-for-me/CLIProxyAPI/blob/928478e4b91533cec05a763bfac3edad9c3e76cf/internal/auth/codex/token.go)、
> [文件元数据](https://github.com/router-for-me/CLIProxyAPI/blob/928478e4b91533cec05a763bfac3edad9c3e76cf/internal/watcher/synthesizer/file.go)；
> Sub2API 的 [导出结构](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/handler/admin/account_data.go)、
> [导入与身份 fallback](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/handler/admin/account_codex_import.go)、
> [可选 account Header](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/service/openai_chatgpt_headers.go)。

## 先导入草稿，再绑定渠道

Console 高级导入页先在浏览器解析草稿（最多 20 个文件，每个 5 MiB）。检查账号、Token 和维护代理映射后再提交。文件和导出 Bundle 含明文 OAuth 或代理凭据，请按密钥保管。账号绑定、路由与授权见[连接账号](../user/codex.md#连接账号)。

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

## 原生 Bundle

`ai-gateway-codex-credentials` 的导入和导出都使用数字版本 `3`。版本不符时不创建凭证或代理草稿。Bundle 包含凭证和可选维护代理；渠道绑定、模型和路由权重由管理员在导入后配置。

## 身份、去重与验证

`access_token`、`refresh_token` 必须有值，`id_token` 可缺；缺少时从 access token 提取身份。
account ID 可缺，但须取得 user ID，解析兼容 auth namespace 中 chatgpt_user_id/user_id
及 JWT 顶层 sub。显式身份与 Token claim 冲突会被拒绝。

浏览器优先按 `(account ID, user ID)`，其次 `(account ID, email)`，accountless 时按
personal user ID，最后才用相同 Token 检测重复草稿。同一 Business workspace 的不同成员
不能仅因 account ID 一样而去重。

服务端独立验证并匹配账号；唯一匹配时更新凭证，不改渠道或路由；多个匹配则拒绝合并。只有 account ID 非空时才发送 `ChatGPT-Account-ID`。

## 差异与限制

- 只导入可识别的 OpenAI Codex OAuth 账号，其他平台/账户类型跳过。
- Sub2API concurrency、priority、rate multiplier、备用代理和过期调度不映射为网关配置。
- CLIProxyAPI last refresh、expired 和未知扩展不直接成为控制面字段。
- 代理必须显式检查映射，不通过导入绕过目标范围或凭证校验。
- 网关只导出自身 Bundle，不生成 CLIProxyAPI/Sub2API 文件，也不承诺无损往返外部产品。
- 维护代理和数据面接入代理不同；导入代理不自动替换已有逻辑渠道的转发网络策略。
