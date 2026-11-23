# 持续集成与安全扫描

> 状态：当前。

每个 PR 和 `main` push 都运行 [CI](../../.github/workflows/ci.yml)。[`ci-changed-areas.sh`](../../scripts/ci-changed-areas.sh) 按实际改动选择 docs、Rust、Console 和 image 检查；未知路径选择全部检查。`ci-gate` 汇总所选任务，是分支保护要求的检查名。

## 检查内容

| 门禁 | 验证什么 |
| --- | --- |
| 文档 | Markdown 链接、结构、元数据以及 diff 格式 |
| Rust | 固定工具链上的 fmt、workspace Clippy 和测试；默认及 `sqlite-backend` 两组都用 PostgreSQL 和[真实插件](connector-plugins.md#验收边界) |
| Console | OpenAPI 类型漂移、TypeScript、lint、组件测试、生产构建及 Playwright 浏览器测试 |
| 系统 | [真实 Gateway、浏览器和 CLI](system-e2e.md)，分别使用临时 PostgreSQL 与 SQLite |
| 镜像 | Docker release 构建与 `--version`。PR 由 image 分类选择；`main` 的 Rust 或 Console 改动也构建镜像 |

[可复用质量工作流](../../.github/workflows/reusable-quality.yml) 运行代码与系统检查。PR 可恢复缓存但不写缓存。同一 PR 的新提交取消旧运行；`main` 按提交保留结果。AMD64 构建缓存由 `main` CI 写入，ARM64 缓存由[镜像预热工作流](../../.github/workflows/release-image-cache.yml)按 image 分类写入。Tag 发布使用对应 `main` SHA 的成功 `ci-gate`，另行执行[发布验证](releasing.md)。

## 安全与排障

PR 的 dependency review 检查新引入的易受攻击依赖；[Security](../../.github/workflows/security.yml) 对 Actions、JavaScript/TypeScript 和 Rust 运行 CodeQL。外部 Action 固定到完整提交 SHA。排查时先选对应事件和提交，再打开最早失败的任务；处理具体告警见[安全告警处置](security-alert-triage.md)。

修改分类或发布自动化时，在本地运行 `scripts/test-ci-changed-areas.sh` 和 `scripts/test-release-automation.sh`。后者使用 Docker Compose CLI 与 PyYAML；所需版本见 CI 工作流。
