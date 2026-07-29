# Codex OAuth Connector 设计记录

> 状态：当前。描述 `connector_kind = codex_oauth` 的已实现架构与扩展约束。

## 决策

特殊上游使用**静态链接、进程内 Connector**，不增加 sidecar、Unix Socket RPC、动态
`.so` 或 WASM。客户端仍使用标准 `POST /v1/responses`；Connector 只改变选中 channel
之后的上游准备过程。

`ApiFormat` 与 `ConnectorKind` 必须分离：

```text
client protocol: OpenAiResponses
upstream connector: CodexOauth
```

因此模型规则、API Key format 权限和 usage 解析仍使用 Responses，不新增
`ApiFormat::CodexResponses`，也不允许 Chat Completions↔Responses 转换。

## 运行时边界

`UpstreamConnectorRegistry` 在进程启动时组装并注入 `ProxyService`。代理主循环只依赖
统一 attempt 接口：

1. `prepare`：在发送前验证动态凭证与 affinity 状态；
2. `adapt_body`：应用 provider 请求约束；
3. `upstream_url`：解析最终目标；
4. `inject_headers`：在通用变换和 hop-by-hop 清理之后注入最终认证；
5. `allows_automatic_retry`：声明 pre-header transport failure 是否可跨 channel 重试；
6. `observe_response`：处理 `401` 等 provider 状态，不延迟客户端响应。

普通 `OpenAiCompatible` attempt 保留原路径、原请求字节和 `UpstreamAuth`。Codex 细节集中在
`src/application/codex/attempt.rs`；`src/application/proxy.rs` 不引用 Codex 类型、路径、
Header 或错误分类。

正式请求直接通过共享 reqwest client streaming 转发。worker 只维护凭证状态，不承载代理流量。

## 持久化模型

`channel_groups.connector_kind` 决定该组使用的 Connector。Codex group 必须使用
`open_ai_responses`，保存后 Connector 类型不可修改。

每个 `codex_oauth_credentials` 记录与一个 managed `channels` 记录一一对应：

- group 仍是凭证池；
- channel `weight`、`proxy_id` 和 `available_models` 继续进入统一路由快照；
- credential 的逻辑 `enabled` 和动态状态由 Connector 快照持有；底层 managed channel
  始终保留为可选择的路由壳，Connector prepare 再排除新 Session 或让 affinity hit fail closed；
- channel 固定 `upstream_auth_kind = none`、`supports_websocket = false`、
  `status_statistics_enabled = false`、`auto_disable_allowed = false`；
- 普通 channel create/update/batch API 在 repository 层拒绝 provider-managed channel；
- provider mutation 在同一控制面事务中更新凭证与 channel、写 audit、编译候选快照并发布。

OAuth PKCE 临时状态单独保存在 `codex_oauth_flows`，按 actor、group、过期时间和
`completed_at` 限定。数据库只保存 `state` 的 SHA-256；`code_verifier` 在一次性 flow
完成或清理前保存。

## 凭证快照与维护

Access token 不编入完整 `CompiledRuntimeConfig`，而是保存在独立
`CodexCredentialRuntime` / `ArcSwap<HashMap<channel_id, credential>>` 中。数据面每次 attempt
只执行内存读取。

worker 每分钟加载数据库记录并先替换本地凭证快照，使其他实例完成的 enable、quota 或 token
更新最终收敛。需要维护的凭证以有界并发执行：

- access token 在过期前 5 分钟刷新；没有 `exp` 时使用保守 fallback age；
- quota 默认 5 分钟刷新；
- 完成或过期的 OAuth flow 清理；
- 单凭证先获取进程内 mutex，再在 PostgreSQL transaction 中
  `SELECT ... FOR UPDATE`，锁内核对 `refresh_generation` 后才调用 token endpoint；
- refresh 事务提交前更新 rotating token、generation、identity 和错误状态；取消或失败时
  transaction drop 会释放 row lock。

worker、上游 `401` 恢复和多实例并发均传递 observed generation；如果其他执行者已经成功轮换，
后续执行者直接结束，不能再次消费旧 refresh token。管理员显式手动刷新不带 observed generation，
因此表示一次强制刷新。

永久 refresh 失败设置持久的 `reauth_required`，maintenance 不再自动重复消费该 Token，quota
成功和普通设置更新也不能清除状态。再次 OAuth 或导入相同 group/account 的新 Token 会事务内更新
原 credential/channel、递增 generation 并清除 `reauth_required`，不会创建重复 channel。

## Quota 与 Session 粘性

Quota 状态：

| 状态 | 新 Session | affinity hit |
| --- | --- | --- |
| `active` | 允许 | 允许 |
| `draining` | 拒绝并排除该 channel 后重选 | 允许 |
| `unavailable` | 拒绝 | fail closed |
| `disabled` | 拒绝并排除该 channel 后重选 | fail closed |

Connector prepare 发生在上游发送前。非 affinity hit 遇到不可用凭证时，代理把当前 dense
channel slot 加入排除集合并重新使用统一路由器；排除集合先使用固定 inline 容量，只有凭证池超过
普通重试上限时才分配 overflow `Vec`。这样标准请求保持无分配路径，而大凭证池仍能遍历到可用账户。

Affinity binding 只在成功终态后写入。首次选择后若凭证进入 `draining`，同一个 Session 仍可继续；
新 Session 不会绑定到 draining 凭证。若已绑定凭证变为 unavailable/disabled/expired，不自动换
账户，也不会因本次失败删除 affinity；绑定会保留到正常 TTL/清理边界，以免后续请求静默切换
provider 账户。

客户端 `session-id` / `thread-id` 优先保留。缺失时，匹配 affinity 的请求从 session hash 加
domain separation 派生稳定 opaque UUID；无 affinity 时生成本次请求 UUID。

## 请求与重试边界

Codex attempt：

- 要求 SSE streaming；
- 强制 `stream=true`、`store=false`；
- 拒绝非空 `previous_response_id`；
- 目标固定为 managed channel base URL 下的 `/responses`；
- 注入 Bearer、`ChatGPT-Account-ID`、可选 FedRAMP、session/thread、User-Agent、
  `originator` 和版本 Header。

preparation 失败可以在发送前换凭证。Codex attempt 不启用普通 transport retry，因为 reqwest
返回 pre-header error 时不能证明请求体未被上游接收。收到任何上游响应头后同样不切换 channel。
`401` 响应原样返回，同时异步触发 generation 去重 refresh。

## 安全边界

- Console 只返回 token 元数据，不返回保存的 ID/access/refresh token。
- credential `Debug`、audit before/after 和错误摘要必须脱敏。
- callback URL、authorization code 和导入 token 不进入日志或浏览器持久化状态。
- 当前 token 与普通 upstream API key 一样以数据库明文列保存；部署者必须保护 PostgreSQL、备份、
  主机和 Console 管理权限。若未来增加列级加密，应使用明确的进程主密钥配置和轮换设计，不能在
  Connector 内临时引入不可恢复的本地密钥。
- OAuth 外部语义不是网关保证；见
  [Codex OAuth 与订阅后端接入参考](../reference/codex-oauth-connect.md)。

## 新增下一种 Connector

新增 provider 时：

1. 增加 `ConnectorKind` 和 group/channel 编译校验；
2. 在独立 provider 模块实现 attempt 与动态凭证运行时；
3. 向 `UpstreamConnectorRegistry` 注册，不修改标准 Connector 行为；
4. 使用 managed channel 复用统一路由、代理、权限、日志和计费；
5. 增加 provider migration、Console OpenAPI、生成类型和独立管理页；
6. 明确 streaming、发送后重试、Session affinity、WebSocket 和 secret 存储边界；
7. 添加协议 mock、数据库事务、端到端转发、quota/draining、并发 refresh 和脱敏测试；
8. 在 `docs/reference/` 记录权威来源、核对日期和外部变化检查项。
