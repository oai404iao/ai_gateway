# 版本发布流程

本项目使用 Semantic Versioning 和 `v<version>` Git tag。`Cargo.toml` 是产品
版本的主来源，但发布门禁要求以下位置保持一致：

- `Cargo.toml`
- `Cargo.lock`
- `tools/forwarding-perf/Cargo.toml`
- `web/console/package.json`
- `docker-compose.prd.yaml` 中的默认 `AI_GATEWAY_VERSION`
- `deploy/compose/env.example`
- `CHANGELOG.md` 中带日期的版本条目

## 发布前准备

1. 从最新 `main` 创建发布变更。
2. 更新上方所有版本位置。
3. 在 `CHANGELOG.md` 将变更归入目标版本，并使用 `YYYY-MM-DD` 日期。
4. 确认工作树只包含发布相关变更，并通过代码评审。
5. 确认 GitHub Actions 已启用。发布工作流使用仓库自动提供的
   `GITHUB_TOKEN` 创建 GitHub Release 并推送 GHCR，不需要额外配置 Registry
   用户名或 PAT。

版本一致性可单独检查：

```bash
./scripts/check-release-version.sh 0.1.0
```

## 本地发布门禁

```bash
./scripts/verify-release.sh 0.1.0
```

该脚本执行：

- Rust format、clippy、完整测试。
- Console API 类型漂移检查、TypeScript、lint、组件测试与生产构建。
- 启用 `embedded-console-ui` feature 的 Rust lint/测试。
- Production Compose 解析。
- 完整 Docker 镜像构建与容器内 `--version` smoke test。

PostgreSQL 集成测试要求先启动 `docker compose up -d`。性能 Harness 仍然只能在
用户明确要求时运行；发布门禁不会执行它。只有转发路径发生变化时，才按
`docs/real-upstream-smoke.md` 额外运行付费真实上游 smoke test。

## 创建并发布 tag

提交发布变更后，确保工作树干净：

```bash
./scripts/release.sh 0.1.0 --push
```

该命令会重新运行发布门禁，创建 annotated tag `v0.1.0`，然后通过 atomic push
同时推送 `main` 与 tag。没有 `--push` 时只创建本地 tag。

已发布 tag 不得移动、覆盖或复用。发现问题时发布新的 patch 版本；不要强推旧 tag。

## GitHub Actions 发布产物

`.github/workflows/release.yml` 在 `v*.*.*` tag 推送后：

1. 再次校验 tag、代码版本与 Changelog。
2. 构建嵌入 Console UI 的 release 二进制。
3. 生成 Linux release tarball 与 `SHA256SUMS`。
4. 构建 `linux/amd64`、`linux/arm64` OCI 镜像。
5. 推送到 `ghcr.io/oai404iao/ai_gateway` 的精确版本 tag；稳定版本还推送
   `major.minor`、`major` 与 `latest`。
6. 使用 Actions 自动注入的 `GITHUB_TOKEN` 创建 GitHub Release 并上传资产。

首次发布的 GHCR Package 默认是私有的，并与源码仓库分别管理可见性。仓库未来
转为 Public 时，还需在 Package 设置中单独确认镜像是否公开。已经发布的 GitHub
Release 不会被工作流覆盖；修复发布问题应使用新的 patch 版本。

迁移时推送的旧 tag 不会补发 GitHub Release，因为对应历史提交中还没有
`.github/workflows/release.yml`。首次自动发布应创建一个包含该工作流的新版本 tag。

## 发布后验证

```bash
git ls-remote --tags origin refs/tags/v0.1.0

docker pull ghcr.io/oai404iao/ai_gateway:0.1.0
docker run --rm ghcr.io/oai404iao/ai_gateway:0.1.0 --version
```

随后在隔离环境按 `docs/production-deployment.md` 启动完整 Compose，验证
`/health`、Console 登录、migration、spool 与数据库投影。私有 Package 需要先
登录 GHCR；也可以从对应 tag 本地构建。
