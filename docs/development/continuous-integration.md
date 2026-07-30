# 持续集成与安全扫描

> 状态：当前。

## 普通 CI

`.github/workflows/ci.yml` 对每个 Pull Request 和 `main` push 运行，并始终
生成稳定的 `ci-gate` 检查。`scripts/ci-changed-areas.sh` 根据变更路径选择门禁：

- Markdown、`docs/` 和 Agent 指令运行文档检查。
- Rust、migration、测试和 workspace 文件运行默认工具链与 MSRV 门禁。
- `web/console/` 和 Console OpenAPI 变更运行类型检查、lint、组件测试、构建与
  Playwright E2E。
- 生产源码、Console、Docker 和部署材料变更运行容器构建与 `--version` smoke。
- `.github/`、`scripts/` 或未知路径采用保守策略，运行全部门禁。

`changes` job 同时对完整变更范围执行 `git diff --check`。文档 job 执行
`python3 scripts/check-docs.py`，因此纯文档 PR 也拥有可作为分支规则必需检查的
`ci-gate`。

Docker image job 只依赖快速的路径分类，与 Rust、Console 和 E2E 并行运行。
最终 `ci-gate` 只接受 `success` 或因路径无关而产生的 `skipped` 结果。

## 并发策略

同一 PR 的新提交会取消旧 CI；`main` push 使用提交 SHA 作为并发键，每个合并
提交都保留完整运行记录。Release 和定时安全扫描使用独立并发组，不会被普通 CI
替代。

## 可复用质量门禁

`.github/workflows/reusable-quality.yml` 是普通 CI 与 Release 共用的质量门禁，
包含：

- 文档检查；
- Rust 1.97.1 format、Clippy 与 workspace 测试；
- Rust 1.92.0 MSRV check 与 workspace 测试；
- Console API 类型漂移、TypeScript、lint、组件测试和生产构建；
- Chromium Playwright E2E，并在失败时上传 trace/test results。

普通 CI 按路径传入启用项。Release 调用相同 workflow 执行 Rust、Console 和
E2E，再由 `.github/workflows/release.yml` 的 `verify` job 完成版本检查、
embedded Console 构建测试和发布资产打包。

## Cache 写入策略

Pull Request 只能恢复默认分支或 Release 已有 cache，不能创建 cache：

- `Swatinem/rust-cache` 通过 `save-if` 限制写入。
- pnpm 使用 `actions/cache/restore`，只有获准的 Console job 调用
  `actions/cache/save`。
- PR Docker 构建只设置 `cache-from`，`cache-to` 为空。

只有 `main` 普通 CI 和 tag-triggered Release 可以写入 cache。稳定 Rust 与
MSRV 使用独立 shared key；Docker 使用 `ci-image-<arch>` 和
`release-image-<arch>` scope。

## 依赖与代码扫描

普通 PR 的 `dependency-review` job 阻止引入 high 或 critical 严重度的已知漏洞
依赖，并显示可用修复版本。

`.github/workflows/security.yml` 在代码 PR、`main` push、每周定时任务和手动
触发时运行 CodeQL `security-extended` 查询：

- GitHub Actions workflow；
- JavaScript/TypeScript Console；
- 使用 CodeQL `none` build mode 提取的 Rust workspace。

GitHub 仓库设置同时启用 Dependabot alerts/security updates、secret scanning
和 push protection。所有外部 Action 必须继续固定到完整 commit SHA。

## `main` ruleset

默认分支 ruleset 要求：

- 禁止删除和非 fast-forward 更新；
- 所有变更通过 Pull Request；
- 允许 squash 或明确需要的 merge commit，不允许 rebase merge；
- 所有 review conversation 已解决；
- `ci-gate` 在最新 `main` 基础上通过。

Solo maintainer 阶段审批数为零。仓库所有者只保留 Pull Request 内的恢复性
bypass；正常变更仍必须走 CI 和项目合并策略。

## 本地验证

修改 workflow、缓存策略或 CI 脚本时至少执行：

```bash
shellcheck scripts/ci-changed-areas.sh
git diff --check
python3 scripts/check-docs.py
pnpm --dir web/console e2e
cargo build --locked --workspace --all-targets
```

Docker 或 Release 路径变更还需执行验证矩阵中的容器/发布检查。
