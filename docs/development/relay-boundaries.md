# 网关、转发与插件的拆分边界

> 状态：当前。本文说明原生插件宿主、HTTP 请求构造与派发的职责边界。

**决策：先拆编译依赖，不拆部署。** 这里有三个不同的问题：一种客户端操作如何校验，一条逻辑请求如何交付，以及一个供应商如何构造上游请求。它们变化的原因不同，不应该按供应商复制整条网关链路，也不需要为每一层创建一个服务。

## 两个上游的取舍

以下为 2026-11-19 核对的固定版本源码，不是性能对比。

| 项目 | 一次请求怎么走 | 借鉴与取舍 |
| --- | --- | --- |
| new-api `3a407cc` | [Relay](https://github.com/QuantumNous/new-api/blob/3a407cc9b064fe79a4ffac4146b2690c465dc929/controller/relay.go) 根据操作选择 helper；[GetAdaptor](https://github.com/QuantumNous/new-api/blob/3a407cc9b064fe79a4ffac4146b2690c465dc929/relay/relay_adaptor.go) 选择渠道适配器。[provider plugin adaptor](https://github.com/QuantumNous/new-api/blob/3a407cc9b064fe79a4ffac4146b2690c465dc929/relay/channel/providerplugin/adaptor.go) 复用声明式 route，叠加凭证与请求钩子；这与改写入口请求的 [request plugins](https://github.com/QuantumNous/new-api/blob/3a407cc9b064fe79a4ffac4146b2690c465dc929/relay/requestplugin/relay.go) 不同。 | 借鉴“操作”和“供应商”两个选择点。但它的 [Adaptor](https://github.com/QuantumNous/new-api/blob/3a407cc9b064fe79a4ffac4146b2690c465dc929/relay/channel/adapter.go) 暴露多种格式转换和 `DoRequest`；我们按操作声明能力，不向原生插件交出 dispatch，也不让它在入口校验前改写请求。 |
| magpie `9768b38` | [Provider.Do → plugin.Fetch](https://github.com/yetone/magpie/blob/9768b384e4d813f809aec5e23dd90292bfc00bf7/docs/subsystems/provider-plugins.md) 将已迁移供应商的请求交给 [Bun 插件宿主](https://github.com/yetone/magpie/blob/9768b384e4d813f809aec5e23dd90292bfc00bf7/internal/plugin/host.go)；跨供应商的 [gateway middleware](https://github.com/yetone/magpie/blob/9768b384e4d813f809aec5e23dd90292bfc00bf7/docs/subsystems/gateway-middleware.md) 则在 Go 进程内运行。 | 借鉴“谁拥有供应商语义”与“谁可以改公共策略”分开。这里的出站代理、连接池、WS continuation 和计量都属于网关；不引入 Bun、插件自有 fetch 或任意顺序的中间件链。 |

## 一个二进制，三个代码边界

```text
编译：ai-gateway → connector-host → connector-sdk
      ai-gateway → connector-sdk ← Codex 插件（独立仓库）
运行：网关选择一个渠道 → connector-host 调用该代际的 .so → 网关校验结果并派发
```

- **SDK** 定义 C 函数表、命令和数据所有权；外部仓库依赖固定版本的 SDK，不链接网关或宿主。
- **[connector-host](../../crates/connector-host/)** 管理制品、加载、能力预检和不可变代际。只在此层持有原生函数指针并调用插件。它不依赖 Axum、reqwest、SQLx 或业务路由，可以单独构建和测试。这是编译边界，不是沙箱；不可信的 `.so` 仍不能启用。
- **网关应用** 绑定已选渠道、凭证及插件代际，校验插件产生的计划，拥有网络副作用。例如 `attempt.target` 返回的 URL 仍受已配置的上游范围约束；插件得不到 `reqwest::Client`、数据库事务或结算写权限。详细命令见 [SDK 契约](../../crates/connector-sdk/docs/commands.md)。原来从 `ai_gateway::connector_plugins` 导入类型的 Rust 调用方改用 `ai_gateway_connector_host`；已构建的插件不需要重编。

选择扩展方式时，从最便宜的机制开始：普通 OpenAI 兼容上游只配置 `general` 渠道与凭证；静态字段/Header 差异用现有[受限变换](transform-dsl.md)；需要供应商签发流程、身份归一化或按内容解析响应时才用连接器。新增客户端操作则属于宿主：要有自己的校验、授权、能力与计量语义，不以模板偷偷转为另一种格式。

## HTTP 与 WebSocket 共用候选准备，而不共用发送器

[`proxy_authenticated_parts`](../../src/application/proxy.rs) 管理 HTTP 请求的准入、选路、发送及失败处置。HTTP 和 WS 选定候选后，走[同一套 JSON 和 Header 准备逻辑](../../src/application/proxy/candidate.rs)：模型改写、渠道 Transform、连接器适配。两者的认证后 Header 清理也在同一个连接器出口执行。最后才按传输区分结果：[`HttpRequestInput::build_for`](../../src/application/proxy/http_attempt.rs) 返回可以发送的 HTTP 请求；[`prepare_upstream_attempt`](../../src/application/proxy/websocket.rs) 返回消息和握手所需的连接池键。multipart 重建和 HTTP 压缩不会进入 WS 消息路径。

| 生命周期 | 接口与所有权 | 验收条件 |
| --- | --- | --- |
| 每条逻辑请求一次 | 入口持有一次快照、过滤后的原始 body、准入和日志预留；候选切换不重读客户端 body。 | 第二次尝试从原始输入开始，不重复预留或结算。 |
| 每个候选一次 | 构造函数接收已选渠道、wire model 与本次 connector；返回绑定复用 client、目标、Header 和 body 的 `reqwest::RequestBuilder`，以及实际超时。准备失败时返回分阶段的错误，不发送请求或决定重试。 | 保持模型别名、Transform、插件、压缩、Header guard 顺序及原始字节直通；任一步拒绝时该候选上游收到零请求。 |
| 派发与完成 | 调用方通过共享 client 发请求，按现有规则在下游提交前决定是否换候选；提交后将响应流和完成记录一起移交。 | 分别覆盖可重试失败、Images/Codex 不可重试、SSE 中断、已观察 usage 和唯一终态。WS 单独验收精确连接命中及消息发送后不可重试。 |

这个切法不需要新 HTTP 端点，也不改变 `/v1/*`、Console OpenAPI、数据库 schema 或 ABI。破坏性变更只有在能移除真实约束时才值得做，例如引入插件故障隔离时，必须以可取消、有背压的 IPC 取代同步函数调用，并重新定义断连后上游是否已派发。仅把 Console 和转发拆成两个进程，还需先解决跨进程快照发布与插件目录独占；不能让两个进程各自假定拥有同一份 SQLite/spool。上述条件出现前保留独立 listener 和单进程部署。
