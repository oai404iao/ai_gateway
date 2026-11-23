# 网关、转发与插件的拆分边界

> 状态：当前。

宿主选择客户端操作和渠道，将供应商协议准备交给连接器，自己完成出站校验与派发。为何拆分编译依赖而不拆部署见[宿主边界决策](../adr/connector-host-boundary.md)。

## 一个二进制，三个代码边界

```text
编译：ai-gateway → connector-host → connector-sdk
      ai-gateway → connector-sdk ← Codex 插件（独立仓库）
运行：网关选择一个渠道 → connector-host 调用该代际的 .so → 网关校验结果并派发
```

- **SDK** 定义 C 函数表、命令和数据所有权；外部仓库依赖固定版本的 SDK，不链接网关或宿主。
- **[connector-host](../../crates/connector-host/)** 管理制品、加载、能力预检和不可变代际。只在此层持有原生函数指针并调用插件。它不依赖 Axum、reqwest、SQLx 或业务路由，可以单独构建和测试。这是编译边界，不是沙箱；不可信的 `.so` 仍不能启用。
- **网关应用** 绑定已选渠道、凭证及插件代际，校验插件计划，拥有网络副作用。`attempt.target` 返回的 URL 仍受已配置的上游范围约束；插件命令不接收 `reqwest::Client`、数据库事务或结算写权限。见 [SDK 契约](../../crates/connector-sdk/docs/commands.md)。

选择扩展方式时，从最便宜的机制开始：普通 OpenAI 兼容上游只配置 `general` 渠道与凭证；静态字段/Header 差异用现有[受限变换](transform-dsl.md)；需要供应商签发流程、身份归一化或按内容解析响应时才用连接器。新增客户端操作则属于宿主：要有自己的校验、授权、能力与计量语义，不以模板偷偷转为另一种格式。

## HTTP 与 WebSocket 共用候选准备，而不共用发送器

[`proxy_authenticated_parts`](../../src/application/proxy.rs) 管理 HTTP 请求的准入、选路、发送及失败处置。HTTP 和 WS 选定候选后，走[同一套 JSON 和 Header 准备逻辑](../../src/application/proxy/candidate.rs)：模型改写、渠道 Transform、连接器适配。两者的认证后 Header 清理也在同一个连接器出口执行。最后才按传输区分结果：[`HttpRequestInput::build_for`](../../src/application/proxy/http_attempt.rs) 返回可以发送的 HTTP 请求；[`prepare_upstream_attempt`](../../src/application/proxy/websocket.rs) 返回消息和握手所需的连接池键。multipart 重建和 HTTP 压缩不会进入 WS 消息路径。

| 生命周期 | 接口与所有权 | 验收条件 |
| --- | --- | --- |
| 每条逻辑请求一次 | 入口持有一次快照、过滤后的原始 body、准入和日志预留；候选切换不重读客户端 body。 | 第二次尝试从原始输入开始，不重复预留或结算。 |
| 每个候选一次 | 构造函数接收已选渠道、wire model 与本次 connector；返回绑定复用 client、目标、Header 和 body 的 `reqwest::RequestBuilder`，以及实际超时。准备失败时返回分阶段的错误，不发送请求或决定重试。 | 保持模型别名、Transform、插件、压缩、Header guard 顺序及原始字节直通；任一步拒绝时该候选上游收到零请求。 |
| 派发与完成 | 调用方通过共享 client 发请求，按现有规则在下游提交前决定是否换候选；提交后将响应流和完成记录一起移交。 | 分别覆盖可重试失败、Images/Codex 不可重试、SSE 中断、已观察 usage 和唯一终态。WS 单独验收精确连接命中及消息发送后不可重试。 |
