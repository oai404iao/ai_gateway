# Codex 凭证导入格式兼容性

> 类型：外部参考
>
> 最近核对：2026-08-03
>
> 权威来源：
> [CLIProxyAPI Codex Token 定义](https://github.com/router-for-me/CLIProxyAPI/blob/928478e4b91533cec05a763bfac3edad9c3e76cf/internal/auth/codex/token.go)、
> [CLIProxyAPI 文件元数据合成](https://github.com/router-for-me/CLIProxyAPI/blob/928478e4b91533cec05a763bfac3edad9c3e76cf/internal/watcher/synthesizer/file.go)、
> [Sub2API 数据导出结构](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/handler/admin/account_data.go)、
> [Sub2API Codex 导入与身份 fallback](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/handler/admin/account_codex_import.go)、
> [Sub2API 可选 account Header](https://github.com/Wei-Shaw/sub2api/blob/825ca7b1fc9335f904bc077f051de815fb61e47f/backend/internal/service/openai_chatgpt_headers.go)

## 外部格式关键语义

CLIProxyAPI 将一个 Codex OAuth 账户保存为 JSON 对象，核心字段为 `type = "codex"`、
`id_token`、`access_token`、`refresh_token`、`account_id` 和 `email`。其他顶层元数据可以包含
`proxy_url`。单个对象和对象数组都是常见的搬运形态。

Sub2API 管理员数据导出使用 `type = "sub2api-data"`、`version = 1`、`proxies` 和 `accounts`。
代理以 `proxy_key` 关联；Codex 账户通常为 `platform = "openai"`、`type = "oauth"`，敏感字段位于
`credentials`，其中 account ID 可能写作 `chatgpt_account_id`。部署或客户端也可能把该 payload
包在带 `code` 和 `data` 的 API 响应封装中。

当前 Sub2API 导入不会把 `chatgpt_account_id` 当作 OAuth Token 的必填字段：缺少时继续使用
`chatgpt_user_id`/`user_id`，并可回退 JWT 顶层 `sub`、email 或 Token 指纹进行身份匹配。其请求
Header helper 同样只在 account ID 非空时写入 `chatgpt-account-id`。因此缺少 account ID 不是
Free plan 专属格式，也不应由导入器补造 workspace ID。

这些是外部项目在上述固定 commit 中的结构，不是它们未来版本的稳定性承诺。

## ai-gateway 兼容行为

高级导入页接受粘贴 JSON 和最多 20 个 JSON 文件，每个文件最大 5 MiB，并自动识别：

- ai-gateway 原生 `ai-gateway-codex-credentials` version 1 Bundle；
- CLIProxyAPI 单对象或对象数组；
- Sub2API 原始数据 payload 或常见 `data` 响应封装；
- 仅含 Token 字段的单个通用对象。

解析发生在浏览器内，结果必须先进入可检查和修改的草稿态。管理员可以修改 label、enable、
account ID、user ID、Token、weight、quota threshold 和代理分配；导入文件中的代理必须先映射到
现有代理，或在同一页面检查并创建。最终每条凭证仍由 ai-gateway 服务端验证 Token、
可选 workspace/member 身份和 Codex models 后写入。

`id_token` 可缺失；此时服务端从 `access_token` 读取身份声明。`access_token` 和
`refresh_token` 必须存在。account ID 可以缺失，但此时必须能取得 user ID；解析兼容
`chatgpt_user_id`、同 namespace 的 `user_id` 和 JWT 顶层 `sub`。浏览器优先按
`(account ID, user ID)`，其次按 `(account ID, email)`；没有 account ID 时按 personal user ID，
最后才按相同 Token 检测重复草稿。不会仅因两个 Business 成员共享同一 workspace account ID
就把后者标成重复。服务端验证 models、quota 和后续数据面请求时，仅在 account ID 存在时发送
`ChatGPT-Account-ID`。

ai-gateway 的导出只生成自己的 versioned Bundle，不尝试生成 CLIProxyAPI 或 Sub2API 文件。Bundle
可包含凭证引用的代理定义和代理认证信息，并保留 account/user ID、enable、weight 和 quota
threshold。

## 差异与限制

- 只导入可识别为 OpenAI Codex OAuth 的账户；其他 Sub2API 平台或账户类型会跳过。
- Sub2API 的 concurrency、priority、rate multiplier、备用代理和过期调度等产品专属字段不会映射。
- CLIProxyAPI 的 last refresh、expired 和未知扩展元数据不会作为 ai-gateway 控制面字段保存。
- 未映射的外部代理不会自动绕过审查创建；管理员可以显式选择直连，但页面会保留警告。
- 外部 JSON 和 ai-gateway 导出都包含明文 OAuth/代理凭据，应按密钥处理，不应提交到版本控制。

## 维护检查项

升级兼容范围时应重新核对上述固定来源，更新本页日期，并至少覆盖：

1. CLIProxyAPI 单对象、对象数组、缺少 `id_token` 和带 `proxy_url` 的解析测试；
2. Sub2API 原始 payload、响应封装、accountless personal identity、`proxy_key` 映射和内嵌
   代理认证测试；
3. 原生 Bundle 导出后重新导入的字段保持；
4. 解析失败、重复凭证、禁用或已删除代理，以及逐条导入失败重试。
