# 文档规范

> 状态：当前。适用于仓库根目录 README、`docs/`、`web/console/README.md` 和 `AGENTS.md`。

## 1. 目标

- 让用户、开发者和编码 Agent 能快速找到唯一入口。
- 区分“当前行为”“设计意图”“外部标准”和“历史记录”。
- 避免同一契约在多个文档中重复维护。
- 让路径移动、接口变更和配置变更具有明确的同步清单。

## 2. 分类与目录

### `docs/user/`

记录用户可观察行为：安装、启动、配置、部署、权限、接口调用和运维边界。内容必须以已发布或当前实现为准，不写未实现计划。

### `docs/development/`

记录维护者需要的架构、模块边界、设计约束、测试工具和发布流程。设计文档必须在开头标明状态，例如“当前”“部分实现”“提案”或“已完成设计记录”。

### `docs/reference/`

记录第三方协议和接口的项目相关摘要。必须包含：

- 权威来源链接；
- 最近核对日期；
- 本项目依赖的语义；
- 本项目与外部接口的差异或未支持范围。

不要复制完整第三方 API 参考，也不要把外部参考误写成网关实现保证。

### `docs/archive/`

保存已完成里程碑、废弃方案和仅用于追溯的材料。每份归档文档必须明确声明不能作为当前行为依据。

### `docs/openapi/`

保存机器可读 API 契约。Console API 的权威源文件是 `docs/openapi/console-v1.yaml`；生成文件不得反向成为规范来源。

## 3. 文档状态

文档标题后应尽早给出状态说明：

```markdown
> 状态：当前。描述已经实现并需要持续维护的行为。
```

可用状态：

- **当前**：描述现有行为，代码变更必须同步更新。
- **部分实现**：同时列出已实现和未实现部分。
- **提案**：尚未成为实现承诺。
- **已完成设计记录**：保留决策背景，当前行为仍以代码和契约为准。
- **历史归档**：仅用于追溯。

## 4. 来源与重复规则

- Console API 形状：只在 `docs/openapi/console-v1.yaml` 定义。
- 数据面客户端/Codex Header、顶层 body 字段动作与 Codex 隐私归一化/安全补全：只在
  `docs/reference/request-allowlists.json` 定义；Markdown 只解释动作和维护流程。
- TOML 字段和默认值：以 `src/runtime_config/mod.rs` 为实现来源，`config.example.toml` 和容器模板必须同步。
- 数据库结构：以 `migrations/` 为准；设计文档用于解释，不替代 migration。
- 前端 API 类型：由 OpenAPI 生成，禁止手工编辑。
- 外部 OpenAI 行为：以官方文档为准；本仓库只维护兼容性摘要。
- `README.md` / `README.zh-CN.md` 负责项目概览和最短启动路径，详细说明链接到 `docs/user/`。
- `AGENTS.md` 只保留影响编码决策的操作规则，并链接到详细文档，不复制长篇用户教程。

## 5. 文件与标题

- 文件名使用小写 kebab-case，例如 `request-log-durability.md`。
- 一个文件只解决一个主要主题。
- 一级标题只出现一次，并与文件主题一致。
- 标题层级不能跳级。
- 路径、命令、字段、接口和类型使用反引号。
- 使用相对链接；移动文件时必须全仓搜索旧路径。
- 示例不得包含真实凭据、JWT 私钥、数据库密码或可用 API Key。

## 6. 推荐结构

当前行为文档：

```markdown
# 标题

> 状态：当前。

## 目标或适用范围
## 前置条件
## 行为或步骤
## 错误与边界
## 验证
## 相关文档
```

外部参考文档：

```markdown
# 接口名称

> 类型：外部参考
> 最近核对：YYYY-MM-DD
> 权威来源：...

## 外部接口关键语义
## ai-gateway 兼容行为
## 差异与限制
## 维护检查项
```

## 7. 变更同步矩阵

| 变更 | 必须检查的文档或契约 |
| --- | --- |
| 公共 `/v1/*` 行为 | `docs/reference/request-allowlists.json`、`docs/user/operations.md`、`docs/reference/`、README、相关测试 |
| Console API 形状 | `docs/openapi/console-v1.yaml`、生成类型、Console 测试 |
| TOML 配置 | 两份配置模板、`docs/user/`、`AGENTS.md` |
| 数据库 schema | migration、开发设计文档、相关运维说明 |
| 转发/Streaming | 兼容性参考、真实上游 smoke 文档、`AGENTS.md` |
| Console UI | `web/console/README.md`、开发设计文档、`AGENTS.md` |
| 发布/部署 | 用户部署文档、开发发布文档、打包脚本、README |
| 文档路径 | 全仓旧路径引用、代码注释、脚本和发布包内容 |

## 8. 校验

文档变更至少执行：

```bash
git diff --check
python3 scripts/check-docs.py
```

并检查：

1. 文档中引用的仓库路径都存在。
2. 相对 Markdown 链接可解析。
3. 命令与 `Cargo.toml`、`package.json`、脚本和 CI 一致。
4. “当前”“计划”“归档”没有混写。
5. 外部参考包含核对日期和权威来源。
6. 中英文 README 的关键路径和产品边界一致。
