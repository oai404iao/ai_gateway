# ai-gateway

> **状态：** 当前实现，持续开发中。

[English](README.md) · [文档](docs/README.md) · [发行版](https://github.com/oai404iao/ai_gateway/releases)

`ai-gateway` 是自托管的单二进制 Rust 网关，用于转发 OpenAI 兼容的 LLM 请求。
你可以配置上游和路由，通过独立 Console 管理访问权限，并以耐久异步流水线记录用量和结算费用。

支持 Chat Completions、Responses HTTP/SSE 与 WebSocket、非流式 Images generation、
multipart Images edit，以及 Codex standalone search 扩展。不同 API 格式始终独立，
不做跨格式转换；具体操作和限制见[兼容性参考](docs/reference/openai-compatibility.md)。

## 开始使用

按[快速上手](docs/user/quickstart.md)启动 PostgreSQL、配置 Console、创建首个管理员与路由，
再发送请求。空控制面能够通过健康检查，但还不能代理请求。

根据部署目标选择入口：

| 目标 | 阅读 |
| --- | --- |
| 在单台主机运行发行镜像 | [生产 Compose 部署](docs/user/production-deployment.md) |
| 在 Linux 使用本地数据库文件 | [SQLite 部署与停机恢复](docs/user/sqlite.md) |
| 调整限制、存储与容量 | [生产配置](docs/user/production-configuration.md) |
| 接入 Codex 订阅 | [安装连接器插件](docs/user/connector-plugins.md) |

默认后端是 PostgreSQL。源码构建使用 SQLite 需要 `sqlite-backend` feature；官方镜像和
发行二进制包含该功能。`general` 连接器内置；Codex 需要从
[ai-gateway-connectors](https://github.com/oai404iao/ai-gateway-connectors)
单独安装并显式启用原生插件，网关发行物不附带该库。

## 使用与运维

- [使用 API 与 Console](docs/user/operations.md)：客户端 Key、路由、流式请求、上游凭证、日志与计费。
- [查阅兼容边界](docs/reference/README.md)：请求字段、协议行为和上游特有的限制。
- [安全运行](docs/user/security.md)：监听器暴露、凭据、可信插件与恢复边界。

公共 `/v1/*` 监听器使用客户端 API Key。Console API 与可选嵌入式 React UI 位于
**另一个监听器**，使用 JWT 会话。生产环境不需要常驻 Node.js；网关不终止 TLS，
Console 应置于 HTTPS 反向代理后。

路由读取不可变内存快照，不逐请求查询控制面数据库。上游客户端复用，普通响应流式转发；
发送下游响应头或任何响应字节后不再重试。RPM、并发、粘性、健康状态和 WebSocket 池
均为进程本地状态，不提供集群级协调。

请求日志不保存 prompt、completion、完整 Header 或凭据，但数据库和备份仍包含敏感控制面数据。
额度是软预检查，不是预付费用预留；将其用于支出限制前，请阅读[运维说明](docs/user/operations.md)。

## 参与开发

先完成[开发环境设置](docs/development/getting-started.md)，再从[维护者文档](docs/development/README.md)
选择主题。[`rust-toolchain.toml`](rust-toolchain.toml) 固定 Rust **1.97.1**；
Console 工具链使用 Node.js 24 和 pnpm 12.10.1。

开发数据库配置并启动后，在仓库根目录运行：

```bash
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

前端命令见 [`web/console/README.md`](web/console/README.md)。Console API 形状由
[`console-v1.yaml`](docs/openapi/console-v1.yaml) 定义，TypeScript 类型必须生成，不能手改。

转发改动还需要经明确授权的[付费真实上游 smoke test](docs/development/real-upstream-smoke.md)。
[性能测试](docs/development/forwarding-performance.md)另行按需执行，二者都不是普通安装步骤。

## 许可证

[GNU Affero General Public License v3.0 only](LICENSE)（`AGPL-3.0-only`）。
修改本程序并通过网络向用户提供修改后的版本时，AGPL 第 13 条要求向这些用户提供获取对应源码的方式。
第三方声明与再分发材料见 [`LICENSES/`](LICENSES/) 和
[`web/console/NOTICES.md`](web/console/NOTICES.md)。
