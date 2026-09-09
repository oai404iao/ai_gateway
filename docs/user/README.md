# 用户文档

> 状态：当前。面向部署者、运维人员和数据面/Console 使用者。

## 推荐阅读顺序

1. 根目录 [`README.zh-CN.md`](../../README.zh-CN.md) 或 [`README.md`](../../README.md)：项目概览和本地快速启动。
2. [运行与接口说明](operations.md)：完整运行时边界、公共数据面、Console、日志和结算行为。
3. [生产配置与容量调优](production-configuration.md)：单节点基线、PostgreSQL、存储和观测。
4. [Docker Compose 生产部署](production-deployment.md)：密钥、启动、升级和回滚边界。

## API 使用者

公共数据面只提供：

- `GET /health`
- `GET /v1/models`
- `POST /v1/chat/completions`
- `POST /v1/responses`
- `POST /v1/alpha/search`
- 带 WebSocket Upgrade 的 `GET /v1/responses`
- `POST /v1/images/generations`
- `POST /v1/images/edits`

接口兼容范围和与 OpenAI 官方语义的关系见 [外部参考文档](../reference/README.md)。

Console API 的请求/响应形状以 [`docs/openapi/console-v1.yaml`](../openapi/console-v1.yaml) 为准。

## 安全提示

- 不要提交 `./config/config.toml`、数据库密码或 Console JWT 私钥。
- Console listener 必须部署在 HTTPS 反向代理后。
- 服务不读取 `.env`；真实上游 smoke test 的忽略文件是唯一例外。
- 数据库和备份包含客户端与上游凭据，应按敏感数据保护。
