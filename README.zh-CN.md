<p align="center">
  <img src="assets/logo.svg" width="96" height="96" alt="ai-gateway 标志">
</p>

# ai-gateway

> 状态：当前。

[English](README.md) · [文档](docs/README.md) · [发行版](https://github.com/oai404iao/ai_gateway/releases)

用 `ai-gateway`，应用可以通过同一个 OpenAI 兼容的 Base URL 调用多个上游。你负责托管网关、签发 API Key，
并决定客户端可见的模型名。在 Console 中，为这些名字配置上游模型、授权使用者并设定用量单价。

## 发出第一条请求

准备好上游的 Base URL、模型 ID 和 API Key，按[本地快速开始](docs/user/quickstart.md)初始化
PostgreSQL，配置第一条路由，再通过网关向上游发送 Chat Completions 请求。

在服务器上运行，可用 [Docker Compose 部署发行镜像](docs/user/production-deployment.md)。
Rust 二进制分别监听公共 API 和管理 Console；请通过 HTTPS 反向代理访问 Console。
数据库默认使用 PostgreSQL，Linux 单机也可以[使用 SQLite](docs/user/sqlite.md)。

**已有旧版实例？** 切换到当前存储基线需要新数据库和独立的空请求日志 spool。请先按[切换与回滚指南](docs/user/upgrades.md)保存原账本。

## 接入客户端与上游

| 客户端操作 | 支持的传输方式 |
| --- | --- |
| Chat Completions | HTTP，支持 SSE 流式响应 |
| Responses | HTTP（支持 SSE 流式响应）或 WebSocket |
| 图片生成与编辑 | 非流式 HTTP；生成使用 JSON，编辑使用 multipart |
| 独立网页搜索 | Codex 扩展，非流式 HTTP |

各操作有独立的[路由规则](docs/user/routing.md)。
内置的 `general` 连接器可接入 OpenAI 兼容上游。使用 Codex 账号时，先[安装并启用原生连接器](docs/user/connector-plugins.md)。

- [调用公共 API](docs/user/data-plane.md)，或查阅[协议兼容范围](docs/reference/openai-compatibility.md)。
- [管理用户与密钥](docs/user/console.md)、[查看请求日志](docs/user/observability.md)、[配置单价与软额度](docs/user/billing.md)。
- 对外提供服务前，先[保护凭证与备份](docs/user/security.md)。
- 要构建或修改网关，先[设置开发环境](docs/development/getting-started.md)。

## 许可证

`ai-gateway` 使用 [GNU Affero General Public License v3.0 only](LICENSE)（`AGPL-3.0-only`）。
用户通过网络与修改版交互时，第 13 条要求向他们提供获取对应源码的方式。
第三方再分发声明见 [`LICENSES/`](LICENSES/) 和 [`web/console/NOTICES.md`](web/console/NOTICES.md)。
