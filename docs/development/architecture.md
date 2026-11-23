# 网关架构

> 状态：当前。

`ai-gateway` 是一个 Rust 进程。公共监听器用 API Key 接受 LLM 请求，Console 监听器用 JWT 管理账户、路由和凭证。二者通过不可变配置快照衔接：控制面发布配置，转发路径在内存中鉴权和选路。

```text
管理员 → Console API → 数据库事务 → 编译并发布快照
                                          ↓
客户端 → 公共 API → 准入 → 选路 → 请求准备 → 上游
                             ↓                ↓
                         持久化 intent      终态与用量 → spool → 计量、结算、查询
```

## 配置与授权

[TOML](../../config.example.toml) 指定监听器、数据库、插件目录和本地存储。模型、路由、渠道、授权和动态运行策略存入数据库。默认后端为 PostgreSQL；Linux 构建可启用[单实例 SQLite](sqlite-backend.md)。Console SPA 可随二进制嵌入，只在 Console 监听器提供，见[前端架构](console-ui.md)。

| 对象 | 决定什么 |
| --- | --- |
| 上游接入 | 连接器、目标 URL、代理和超时 |
| 凭证 | 上游身份、可用目标范围和生命周期 |
| 逻辑渠道 | 绑定接入与凭证，是 Key 授权的单位 |
| 操作能力 | 该渠道可处理的操作、上游模型和请求策略 |
| 客户端模型 | 客户端请求中的模型名及其价格 |
| 模型的操作规则 | 按优先级排列的层；每层显式列出能力、上游模型和权重 |

Key 可直接选择渠道，也可选择组，由快照展开为该组的当前成员。成员变化影响授权，但不会向模型规则添加候选。`/v1/models` 显示此 Key 可达且有匹配上游模型的客户端模型。转发计费取客户端模型的价格，不因切换候选而改变。

Console 写操作验证、审计并提交变更，随后发布完整的 `CompiledRuntimeConfig`；后台重载让其他进程获得更新。一次请求使用同一份路由快照。RPM、并发、被动健康、Session 粘性和 WebSocket 连接池则各自留在处理请求的实例。详见[上游实体与路由](upstream-identity-capabilities.md)及[控制面持久化](database-architecture.md)。

## 从请求到上游

1. **鉴权与准入。** [`src/application/proxy.rs`](../../src/application/proxy.rs) 从快照验证 Key 与权限，检查进程内 RPM、并发和软额度，再读取请求体。
2. **读取与筛选。** JSON 按大小上限读取；Responses HTTP 可先做有界 zstd 解压。multipart Images edit 使用独立限制，超过内存阈值时写入匿名临时文件。[公共请求策略](../reference/request-allowlists.json)筛选顶层字段；若用户组过滤 Fast，先移除 `service_tier`，再提取计费、粘性和变换所用的元数据。
3. **选路。** 以操作和客户端模型查规则，按授权、模型匹配、可用性和粘性筛选候选，从最低可用优先级的层按权重选取。Chat、Responses HTTP、Responses WS、独立搜索、Images generation 和 edit 各自选路。
4. **派发前记录。** 持久化请求 intent，预留终态空间；写入失败返回 `503 request_log_unavailable`，此时尚未派发。自动重试共用这条逻辑请求记录。
5. **准备尝试。** 写入候选的上游模型，按模板及渠道覆盖执行[受限变换](transform-dsl.md)。连接器准备目标、body 与协议 Header，宿主清理 hop-by-hop 和禁止转发的 Header，应用上游认证并在发送前复核。普通请求若无需变换或删改，可保留原始请求字节；Images edit 的截取和回放见[Images 实现](openai-images.md)。
6. **传输与响应。** 宿主按代理、TLS 和超时策略复用 reqwest 客户端。HTTP body 流式转发；用量解析与 SSE 变换读取解码后的上游内容。只有显式启用的[同格式 JSON 适配](connector-response-adapters.md)才有界缓冲完整成功响应，费用仍取原始上游用量。

普通 HTTP 连接失败或命中可重试状态码时，可在向客户端发响应前更换候选；排除的是已尝试的能力/模型组合。Images 和 Codex 派发后不重试。已向客户端发送响应头或数据后，不再切换候选。

## 连接器与 WebSocket

内置 `general` 面向普通 OpenAI 上游。Codex 使用单独安装的原生插件。宿主持有网络、数据库、财务事实和出站限制；插件提供协议准备、凭证解释与供应商隐私策略。插件是受信任的同进程代码。启用、升级与设置变更发布新的不可变代际，在途请求继续使用已选代际。见[连接器边界](connector-plugins.md)及[SDK](../../crates/connector-sdk/README.md)。

Responses WebSocket 在 `GET /v1/responses` 握手时鉴权，每条顺序的 `response.create` 作为独立请求准入、路由、计量。每次请求独占一个上游 socket；只有干净终态才能归还连接池。`previous_response_id` 依赖连接本地状态，未精确命中返回 `404 previous_response_not_found`，由客户端提供完整上下文重试。`426 websocket_unavailable` 提示传输不可用，不会让网关自动重放已发送的请求。详见[WS 状态与恢复](../user/websocket.md)。

## 日志与结算

当前格式的终态先写入本地 spool，再进入数据库 ingress（PostgreSQL COPY / SQLite 批量事务）；收到提交确认才推进 spool checkpoint。worker 将请求时凭证 ID 与用量、价格一同物化为不可变事实，独立处理两类后续工作：

- **结算**在同一事务写唯一回执、变更余额和 Key 额度、移除待结算项。重放同一请求不会再次扣款。
- **投影**写入可查询的请求日志。日志暂不可见不妨碍已验证的事实结算；事实和投影都持久化后，才能确认 ingress。

只有 intent 没有终态时保留待核对，不推断零费用。具体恢复条件见[日志耐久化](request-log-durability.md)和[独立计量](independent-metering.md)。
