# ai-gateway 文档中心

本目录按读者和用途分组。开始阅读前，先选择与你当前任务对应的入口。

## 文档分类

| 分类 | 面向读者 | 内容 |
| --- | --- | --- |
| [用户文档](user/README.md) | 部署者、运维人员、API 使用者 | 启动、配置、部署、数据面和 Console 使用说明 |
| [开发文档](development/README.md) | 后端、前端、测试和发布维护者 | 当前架构、设计约束、测试、性能和发布流程 |
| [外部参考](reference/README.md) | 兼容性开发者、上游接入人员 | OpenAI 接口语义、网关兼容边界和权威外部链接 |
| [OpenAPI 契约](openapi/console-v1.yaml) | Console 后端与前端开发者 | Console API 请求/响应的机器可读权威规范 |
| [请求白名单契约](reference/request-allowlists.json) | 数据面与 Connector 维护者 | 客户端/Codex Header、顶层 body 字段动作和 Codex 隐私归一化/安全补全 |
| [历史归档](archive/README.md) | 追溯历史决策的维护者 | 已完成 MVP 清单；不能作为当前行为依据 |

仓库根目录的 [`AGENTS.md`](../AGENTS.md) 是编码 Agent 的操作手册，不是用户文档或架构文档。

## 来源优先级

当文档之间出现差异时，按以下优先级判断：

1. 当前实现、测试、migration、配置反序列化类型。
2. `docs/openapi/console-v1.yaml`、`docs/reference/request-allowlists.json` 等机器可读契约。
3. 标记为“当前”的用户文档和开发文档。
4. 设计提案、产品蓝图和外部参考。
5. `docs/archive/` 中的历史材料。

外部 API 会持续变化。`docs/reference/` 只记录本项目需要依赖的语义和检查日期，不复制完整第三方文档。

## 维护规范

新增、移动或修改文档前，请阅读 [文档规范](documentation-standard.md)。文档变更至少需要：

```bash
git diff --check
python3 scripts/check-docs.py
```

同时确认所有相对链接、命令、文件路径和状态说明仍然有效。
