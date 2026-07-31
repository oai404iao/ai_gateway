# 外部参考文档

> 类型：外部参考索引。第三方接口可能变化，使用前请检查各文档的“最近核对”日期。

本目录不复制完整 OpenAI 文档，只记录 `ai-gateway` 实现、测试和上游接入所依赖的行为。

## OpenAI API

- [兼容性总览](openai-compatibility.md)
- [Chat Completions](chat-completions.md)
- [Responses](responses.md)
- [Images](openai-images.md)
- [Codex OAuth 与订阅后端接入参考](codex-oauth-connect.md)
- [Codex 凭证导入格式兼容性](codex-credential-portability.md)
- [Codex Responses WebSocket 实现参考](codex-responses-websocket.md)

## 其他外部服务

- [ip-api.com 代理出口 IP 查询](ip-api-proxy-test.md)

## 使用原则

1. OpenAI 官方文档定义外部 API 语义。
2. 本目录定义 `ai-gateway` 选择兼容的范围。
3. `src/` 和集成测试定义当前真正实现的行为。
4. 某个兼容上游如果偏离 OpenAI 语义，应在渠道配置、变换或上游专属文档中显式记录，不能修改通用参考来掩盖差异。

更新外部参考时，应记录核对日期，并优先链接官方 API Reference、官方 Guide 或正式规范。
