# 接入 Codex 账号

> 状态：当前。

要让客户端通过网关使用 ChatGPT 订阅，先[安装并启用 Codex 插件](connector-plugins.md)，再连接账号、配置路由和授权。客户端始终使用 Gateway API Key，不需要拿到订阅 Token。

## 连接账号

1. 在「上游凭证 → Codex」选择 **Connect account**，完成浏览器授权，将以 `http://localhost:1455/auth/callback` 开头的完整回调地址粘回 Console。已有凭证可选择 **Import tokens**；原生 Bundle、CLIProxyAPI 和 Sub2API 文件使用 **Advanced import**，见[导入格式](../reference/codex-credential-portability.md)。
2. 创建 `codex` 上游接入，在渠道组内创建逻辑渠道并绑定这份凭证。维护代理用于 OAuth、Token 和额度操作；请求转发走接入的代理。同一凭证可供多个渠道使用。
3. 在渠道详情添加所需操作能力和模型目录，然后[配置模型候选和 Key 授权](routing.md)。Responses HTTP、Responses WebSocket、独立搜索、图片生成和编辑各用自己的规则。凭证导入或模型发现不会替你发布路由。
4. 将客户端 provider 的 Base URL 指向 Gateway `/v1`，用具备 `proxy` 权限且获渠道授权的 Key 发起请求。需要独立搜索时，客户端还须打开 `supports_standalone_web_search`。

## 识别凭证状态

| 状态 | 如何处理 |
| --- | --- |
| `active` | 凭证可用；能否调用还取决于对应操作的路由和授权。 |
| `draining` | 用量达到设置的阈值，不接新会话。已命中亲和记录的请求仍可继续；Images 不使用亲和。 |
| `unavailable` | 检查额度是否允许请求，以及凭证是否需要重新授权。永久 refresh 失败后，仅查询额度或编辑设置不能恢复。 |
| `disabled` | 凭证已被管理员停用。 |

重新授权或导入同一 workspace/member（个人账号为无 workspace 的同一 user ID）会更新原凭证，保留渠道、模型目录和路由。多个凭证匹配同一身份时不能自动合并。

## 导入、导出与删除

在凭证列表选择账号可批量启用、停用、导出或删除。**Export credentials 导出的是秘密**：原生 Bundle v3 包含原始 ID/access/refresh Token、账号身份与 enable/quota 设置，还可包含所选代理及其认证信息。它不包含路由权重或渠道归属。管理员确认后才可下载，请按未加密的密钥备份保管。常规列表和详情不能读取 Token。

删除凭证前，先解绑全部引用渠道（包括停用渠道）。删除会擦除已存 Token、释放维护代理并保留历史请求引用；如需让外部 Token 失效，还要在账号侧撤销授权。删除代理则须先解除上游接入、凭证维护设置和未清理 OAuth 流的引用。

## 刷新 Token 与查看额度

后台每分钟检查凭证，在 access token 接近过期时刷新；默认每 15 分钟查一次额度。管理员也可在凭证页手动刷新。SQLite 上未确认结果的 refresh/reset 不会自动重试，按[未知结果处理](sqlite.md#升级与未知结果)保留证据。

窗口历史区分按计划换窗的「自然重置」、管理员确认消费 credit 后的「手动 reset credit」，以及根据提前换窗推断的「OpenAI 官方重置」。最后一种是推断而非服务商返回的原因。未使用窗口的 `reset_at` 可能随查询滑动，不会每次都算成重置。网关不会自动消费 credit；手动操作后若额度刷新失败，先等待后台核对，不要重复兑换。

窗口花费汇总该凭证下所有用户的已计价调用，按网关保存的价格和用量计算，不是服务商账单。主次窗口常有重叠，不能把金额相加。普通用户的[额度查看权](console.md#查看-codex-额度)由用户组授予。

## 按操作调用

| 操作 | 客户端需要注意 |
| --- | --- |
| Responses HTTP | `POST /v1/responses` 需要 `stream: true`，不能带非空 `previous_response_id`。插件设置 `store: false`，删除 `max_output_tokens`，因此该值不限制订阅账号的输出。成功响应为 SSE。 |
| Responses WebSocket | 在系统和用户设置中[开启 WebSocket](websocket.md)，配置 `responses-ws` 能力及路由。顺序发送 `response.create`；可用 `previous_response_id` 在原上游连接续写。与 HTTP 一样不保留 `max_output_tokens`。 |
| 独立搜索 | `POST /v1/alpha/search` 返回非流式 JSON。不复用 Responses 路由；没有可识别 usage 时不估算费用。 |
| 图片 | generation 使用 JSON，edit 使用 multipart；Codex edit 最多 5 张输入图且不接受 mask。支持的字段和值见 [Images 兼容范围](../reference/openai-images.md#codex-插件)。两者均不使用 Session 亲和。 |

需要压缩 HTTP Responses 时，在对应能力开启 `zstd`；该设置不影响 WebSocket、搜索或图片。请求一旦发往 Codex 就不自动重试。若想让同一会话的搜索和 Responses 使用同一账号，应为各操作明确选择绑定同一凭证的能力：亲和按模型规则隔离，不跨操作选账号。已命中亲和的凭证不可用时，不会为继续该会话而改用另一账号。

## 会话身份与秘密保护

插件在管理员变换之后处理出站身份：保留允许的会话标识，补全缺失值，以凭证稳定的安装 ID 和合成 Git 工作区替换客户端本地安装信息、路径和 Git 元数据。`originator`、客户端版本、User-Agent 和合成工作区均在[插件设置](connector-plugins.md#3-调整插件设置)中维护，默认值取决于安装的插件版本。

数据库、导出包及其备份需按[秘密保护要求](security.md)管控。出站字段、版本差异及订阅后端行为见 [Codex 接入参考](../reference/codex-oauth-connect.md)。
