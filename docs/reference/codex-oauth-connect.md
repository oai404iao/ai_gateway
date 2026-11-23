# Codex OAuth 与订阅后端

> 类型：固定版本的外部协议参考。
>
> 最近核对：2026-08-19（OAuth 基线）；各接口的源码版本见下文。
>
> 权威来源：[Codex 0.146.0](https://github.com/openai/codex/releases/tag/rust-v0.146.0)、[OAuth 登录服务](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/login/src/server.rs)。

Codex 用 OAuth 访问 ChatGPT 订阅后端。下表中的路径是客户端实现使用的接口，不属于 OpenAI Platform 的公共 API。配置网关、导入账号和查看额度见[接入 Codex 账号](../user/codex.md)。

## OAuth 与账户身份

| 项目 | 核对版本的取值 |
| --- | --- |
| 授权 / Token 端点 | `https://auth.openai.com/oauth/authorize` / `https://auth.openai.com/oauth/token` |
| 授权方式 | Authorization Code，PKCE `S256`，随机 `state` |
| Public client ID | `app_EMoamEEZ73f0CkXaXp7hrann` |
| 回调地址 | `http://localhost:1455/auth/callback` |
| Scope | `openid profile email offline_access api.connectors.read api.connectors.invoke` |

授权码以表单提交，刷新请求使用 JSON。服务可轮换 ID、access 和 refresh token；`refresh_token_expired`、`refresh_token_reused` 和 `refresh_token_invalidated` 需要重新授权。见 [Token 刷新实现](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/login/src/auth/manager.rs)。

`chatgpt_account_id` 是可选的 workspace 身份，`chatgpt_user_id` 标识成员；一个 Business workspace 可有多个成员。请求使用 access token Bearer，仅在有 account ID 时加 `ChatGPT-Account-ID`，FedRAMP 账号按需加 `X-OpenAI-Fedramp`。网关导入时用 claims 匹配身份，再向 models 端点验证；解码 JWT 本身不足以确认凭证可用。见 [Token claims](https://github.com/openai/codex/blob/bb5054fe47abe73ecbbd454751066a28c89f4bb9/codex-rs/login/src/token_data.rs)、[Bearer Header](https://github.com/openai/codex/blob/bb5054fe47abe73ecbbd454751066a28c89f4bb9/codex-rs/model-provider/src/bearer_auth_provider.rs)。

## 后端接口

路径相对于 `https://chatgpt.com/backend-api`。客户端版本、身份和操作共同决定请求；查询模型时的 `client_version` 是向 Codex 声明的版本。

| 接口 | 外部请求与结果 |
| --- | --- |
| `POST /codex/responses` | [Responses 请求](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/common.rs)使用 `stream=true`、`store=false`；结果是 SSE。 |
| `GET /codex/responses` Upgrade | 依次发送 `response.create`，可用 `previous_response_id` 续接本连接上的上下文。见 [WS 请求与恢复](codex-responses-websocket.md)。 |
| `POST /codex/alpha/search` | [Search 类型](https://github.com/openai/codex/blob/5af85998c24fb3353ddd8164c3ed472057b03cb3/codex-rs/codex-api/src/search.rs)有 `id`、`model` 和可选 `reasoning`、`input`、`commands`、`settings`、`max_output_tokens`；非流式结果中 `output` 为最终文本，`encrypted_output` 和 `results` 可缺。 |
| `POST /codex/images/generations`、`POST /codex/images/edits` | [Images 类型](https://github.com/openai/codex/blob/a0a30c40e8dbcc44af52e2df64dcce64368d4422/codex-rs/codex-api/src/images.rs)使用 JSON。edit 的 `images[]` 可用 `image_url` 或 `file_id`；网关公共入口使用 [multipart 上传](openai-images.md#codex-插件)。 |
| `GET /codex/models?client_version=…` | [模型目录](https://github.com/openai/codex/blob/e363b08c9175ac1cbe5893615dd2cb9ddf95043b/codex-rs/codex-api/src/endpoint/models.rs)位于 `models` 数组。网关列出非空且未显式标为 `supported_in_api=false` 的 slug。 |
| `GET /wham/usage` | 返回请求是否获准、主次额度窗口的使用百分比与重置时间，以及可用的 reset credits。 |
| `POST /wham/rate-limit-reset-credits/consume` | [消费协议](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/backend-client/src/client/rate_limit_resets.rs)使用唯一 `redeem_request_id`，结果包括 `reset`、`nothing_to_reset`、`no_credit`、`already_redeemed`。网关由管理员发起消费，不指定 `credit_id`，消费后另行刷新额度。 |

## 网关与插件分别处理什么

客户端使用 **Gateway API Key** 调用 `/v1`；上游 OAuth Token 保存在凭证中。网关选择操作与候选、校验凭证目标、执行网络请求并结算费用。插件准备后端地址、服务商字段与身份，并解析 Token、目录、额度和用量。HTTP Responses 的 `max_output_tokens` 可在公共入口接收，但插件在发送给订阅后端前删除，因此不能用它限制 Codex 输出。

普通 Header 或 JSON 变换执行后，插件再处理 [Codex 请求身份](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/responses_metadata.rs)：保留允许的会话关联，替换客户端 installation ID 和本地 Git 工作区，按最终模型及有效 `service_tier` 派生 routing hint。具体出站字段由[已安装的插件](https://github.com/oai404iao/ai-gateway-connectors)定义；公共入口的字段动作见[请求白名单](request-allowlists.md)。
