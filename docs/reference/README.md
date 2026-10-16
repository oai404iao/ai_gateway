# 外部参考文档

> 类型：外部参考索引。第三方语义按各页的核对日期和固定源码版本理解，不代表实时状态。

先确定你要调用的操作，再查对应兼容边界。本目录不是完整 OpenAI API 手册；部署、授权和
Console 配置步骤见[用户文档](../user/README.md)。

## 按任务查找

| 你要做什么 | 从这里开始 |
| --- | --- |
| 判断 SDK 能否接入、哪些路径已实现 | [OpenAI 兼容性总览](openai-compatibility.md) |
| 排查本地未知字段错误或 Header 丢失 | [请求白名单](request-allowlists.md)及其[机器契约](request-allowlists.json) |
| 接入 messages、工具调用或 Chat SSE | [Chat Completions](chat-completions.md) |
| 接入 Responses JSON、SSE 或 WebSocket | [Responses](responses.md) |
| 接入图片生成或 multipart 编辑 | [Images](openai-images.md) |
| 使用 ChatGPT 订阅凭证访问 Codex 后端 | [Codex OAuth](codex-oauth-connect.md) |
| 从其他管理工具迁移 Codex 凭证 | [凭证格式兼容性](codex-credential-portability.md) |
| 理解 Codex 的增量状态、连接复用和 HTTP fallback | [Codex WebSocket 源码研究](codex-responses-websocket.md) |
| 配置时段计费或检查代理出口 | [DeepSeek 峰谷定价](deepseek-peak-pricing.md)、[ip-api.com](ip-api-proxy-test.md) |
| 研究测试、数据库和耐久日志的工程取舍 | [Monoize 工程研究](monoize-engineering-study.md) |

## OpenAI API

公共数据面当前覆盖以下组合；“支持入口”不等于目标上游支持全部嵌套字段或模型能力。

| 客户端操作 | 请求形状 | 响应方式 | 边界 |
| --- | --- | --- | --- |
| Chat Completions | JSON `messages` | JSON / SSE | 不转换为 Responses |
| Responses HTTP | JSON `input` | JSON / SSE | 与 WS 分别配置操作路由 |
| Responses WebSocket | `response.create` 消息 | 顺序 JSON 事件 | 一次一个在途请求，增量状态连接本地 |
| Standalone search | JSON | 非流式 JSON | Codex 扩展，不是稳定 OpenAI 公共 API |
| Images generation | JSON | 非流式 JSON | 不支持图片流式 |
| Images edit | multipart | 非流式 JSON | 不接受公共 JSON/data URL edit |

完整路径、操作名和错误见[兼容性总览](openai-compatibility.md)。

## 其他外部服务

[DeepSeek](deepseek-peak-pricing.md)记录定价预设的来源，不是实时价格表；
[ip-api.com](ip-api-proxy-test.md)记录诊断依赖、授权和明文 HTTP 风险，不是健康探测保证。

## 外部项目工程研究

[Monoize](monoize-engineering-study.md)与[Codex WebSocket](codex-responses-websocket.md)
保留固定版本观察。研究结论不能直接当作当前网关实现或验收结果。

## 使用原则

1. **外部语义**：官方文档或固定源码定义第三方行为；核对日期不因文档重排自动更新。
2. **网关边界**：当前代码、测试和机器契约定义实际支持范围；允许字段不等于验证嵌套内容。
3. **插件边界**：网关拥有公共入口策略和通用安全约束，服务商协议、隐私和专有字段由插件维护。
4. **历史研究**：保留版本和限制，当前操作入口链接到用户或开发文档，不沿用旧进度清单。

若上游偏离 OpenAI 语义，应显式配置同格式适配或记录服务商差异，不能借此承诺跨协议转换。
