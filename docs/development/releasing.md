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

发布构建使用 `rust-toolchain.toml` 固定的 Rust 1.97.1；源码 MSRV 为 1.92，
并由普通 CI 的独立 job 持续验证。版本职责和约半年的兼容窗口见
[Rust 工具链与 MSRV 策略](rust-toolchain-policy.md)。

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

该命令要求本地 `main` 与 `origin/main` 完全一致，验证目标 SHA 的 `main`
push 已有成功的 `ci-gate`，再创建 annotated tag `v0.1.0`，然后通过 atomic
push 同时推送 `main` 与 tag。发布准备阶段已经运行完整本地发布门禁，因此默认
不会在打 tag 前重复执行；需要人工再次验证时可显式增加 `--verify`。没有
`--push` 时只创建本地 tag。查询 `ci-gate` 需要已认证的 GitHub CLI。

已发布 tag 不得移动、覆盖或复用。发现问题时发布新的 patch 版本；不要强推旧 tag。

## GitHub Actions 发布产物

`.github/workflows/release.yml` 在 `v*.*.*` tag 推送后：

1. `preflight` 要求 annotated tag 精确解析到 workflow SHA，且该提交属于
   `main`，校验代码版本与 Changelog，并通过 GitHub Actions API 固定目标 SHA
   已成功完成 `main` `ci-gate`。本地 `release.sh` 在创建 tag 时进一步要求
   `HEAD` 与 `origin/main` 完全一致。Tag workflow 不再重复运行相同 SHA 已通过
   的普通 Rust、Console 与 Playwright 门禁。
2. `verify` 与两个平台镜像构建在 `preflight` 后并行。`verify` 只执行发布专属的
   embedded Console 构建、Clippy 和 serving tests。
3. `publish-platform-images` 分别在原生 `ubuntu-24.04`
   (`linux/amd64`) 与 `ubuntu-24.04-arm` (`linux/arm64`) runner 上并行构建，
   以平台 digest 推送镜像，避免使用 QEMU 编译 Rust；Dockerfile 中与架构无关的
   Console 构建和 `cargo-chef prepare` 阶段固定在 `$BUILDPLATFORM`。平台 digest
   在门禁结束前没有稳定 tag；失败运行最多留下不可发现的无标签 digest。
4. Docker planner 在生成 cargo-chef recipe 前将 workspace 自身版本规范化为
   固定占位值，因此单纯的 release version bump 不再使第三方依赖层失效。
5. 普通 `main` CI 预热 AMD64 cache；独立的
   `.github/workflows/release-image-cache.yml` 在 `main` 的 image 相关变更后
   异步预热 ARM64 `release-image-arm64` cache。Tag Release 只恢复 cache，不在
   发布关键路径上传 `mode=max` cache。
6. AMD64 平台 lane 从刚推送的镜像中提取二进制和第三方许可证材料，生成包含
   完整 `docs/` 文档树的 Linux tarball、`SHA256SUMS` 与 release notes。该打包
   与 ARM64 构建重叠执行，且 GitHub tarball 与 AMD64 容器使用同一份已编译
   二进制，不再分别编译。
7. `publish-image` job 下载两个平台 digest，生成稳定版或预发布版 tag，并将
   它们合并为一个 multi-platform manifest。
8. Public 仓库额外为最终 multi-platform 镜像生成并推送 GitHub artifact
   attestation；Private 仓库会跳过此步骤。
9. 仅拥有 `contents: write` 的 `publish-release` job 创建 GitHub Release
   并上传资产。

最终 image tags、provenance 和 GitHub Release 仍然必须等待发布专属验证、两个
平台构建及资产打包全部成功。并行化只提前执行无标签构建，不降低发布门禁。

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
