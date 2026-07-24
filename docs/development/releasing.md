# 版本发布流程

> 状态：当前。

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
   用户名或 PAT。所有外部 Action 都固定到完整 commit SHA，并由
   `.github/dependabot.yml` 定期更新带版本 tag 的引用；固定到 `stable` 分支
   commit 的 Rust toolchain Action 需要在常规依赖维护时人工复核。

版本一致性可单独检查：

```bash
./scripts/check-release-version.sh 0.1.0
```

## 本地发布门禁

```bash
./scripts/verify-release.sh 0.1.0
```

该脚本执行：

- Rust format、workspace-wide clippy 与完整测试（包括性能工具的轻量单元测试）。
- Console API 类型漂移检查、TypeScript、lint、组件测试与生产构建。
- 启用 `embedded-console-ui` feature 的 Rust lint/测试。
- Production Compose 解析。
- 完整 Docker 镜像构建与容器内 `--version` smoke test。

PostgreSQL 集成测试要求先启动 `docker compose up -d`。性能 Harness 仍然只能在
用户明确要求时运行；发布门禁不会执行它。只有转发路径发生变化时，才按
`docs/development/real-upstream-smoke.md` 额外运行付费真实上游 smoke test。

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

1. 只读权限的 `verify` job 再次校验 tag、代码版本与 Changelog，并执行完整
   Rust workspace、Console 和 embedded UI 门禁。
2. 构建 release 二进制，生成包含项目许可证和第三方声明的 Linux tarball、
   完整 `docs/` 文档树、`SHA256SUMS` 与 release notes，并以一天保留期暂存为
   Actions artifact。
3. `publish-platform-images` matrix 分别在原生 `ubuntu-24.04`
   (`linux/amd64`) 与 `ubuntu-24.04-arm` (`linux/arm64`) runner 上并行构建，
   以平台 digest 推送镜像，避免使用 QEMU 编译 Rust；Dockerfile 中与架构无关的
   Console 构建和 `cargo-chef prepare` 阶段固定在 `$BUILDPLATFORM`。
4. 平台构建使用独立的 `ci-image-<arch>` / `release-image-<arch>` GitHub
   Actions cache scope；普通 CI 预热 AMD64 cache，成功的 Release 构建分别
   持久化两个架构的 cache。
5. `publish-image` job 下载两个平台 digest，生成稳定版或预发布版 tag，并将
   它们合并为一个 multi-platform manifest。
6. Public 仓库额外为最终 multi-platform 镜像生成并推送 GitHub artifact
   attestation；Private 仓库会跳过此步骤。
7. 仅拥有 `contents: write` 的 `publish-release` job 创建 GitHub Release
   并上传资产。

稳定版本会发布精确版本、`major.minor`、`major` 与 `latest`；预发布版本只发布
精确 SemVer tag。镜像地址为 `ghcr.io/oai404iao/ai_gateway`，OCI
`org.opencontainers.image.licenses` label 固定为 `AGPL-3.0-only`。

首次发布的 GHCR Package 默认是私有的，并与源码仓库分别管理可见性。仓库未来
转为 Public 时，还需在 Package 设置中单独确认镜像是否公开。已经发布的 GitHub
Release 不会被工作流覆盖；修复发布问题应使用新的 patch 版本。

迁移时推送的旧 tag 不会补发 GitHub Release，因为对应历史提交中还没有
`.github/workflows/release.yml`。首次自动发布应创建一个包含该工作流的新版本 tag。

仓库转为 Public 后应在 GitHub Release 设置中启用 Immutable Releases。当前发布
脚本本身也拒绝覆盖已有 Release；修复问题必须发布新的 patch 版本。

## 发布后验证

```bash
git ls-remote --tags origin refs/tags/v0.1.0

docker pull ghcr.io/oai404iao/ai_gateway:0.1.0
docker run --rm ghcr.io/oai404iao/ai_gateway:0.1.0 --version
```

随后在隔离环境按 `docs/user/production-deployment.md` 启动完整 Compose，验证
`/health`、Console 登录、migration、spool 与数据库投影。私有 Package 需要先
登录 GHCR；也可以从对应 tag 本地构建。
