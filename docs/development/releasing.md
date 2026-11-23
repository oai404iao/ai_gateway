# 版本发布流程

> 状态：当前。

发布以 `main` 上的 annotated `v<version>` tag 为入口。版本准备 PR、该提交的 `ci-gate` 和 tag 构建分别验证源码、合并结果与发行产物。

## 准备版本

在分支上更新 `Cargo.toml`、`Cargo.lock`、`tools/forwarding-perf/Cargo.toml`、`web/console/package.json`、`docker-compose.prd.yaml` 的镜像和构建版本、`deploy/compose/env.example` 以及 `CHANGELOG.md` 的带日期条目。

先按[测试准备步骤](getting-started.md)导出 `TEST_DATABASE_ADMIN_URL` 和 `AI_GATEWAY_TEST_CODEX_PLUGIN`。本地需要 Docker 与 Compose、固定的 Rust 和 Console 工具链。

```sh
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
./scripts/check-release-version.sh "$VERSION"
./scripts/verify-release.sh "$VERSION"
```

[验证脚本](../../scripts/verify-release.sh)检查 Rust 和 Console、嵌入式 UI、Compose、容器版本与随附许可证。转发变更还需按[真实上游 smoke](real-upstream-smoke.md)验证。通过后提交版本准备 PR。

## 创建 tag

确认发布准备 PR 已合入、工作树干净、`main` 与 `origin/main` 一致，并用已认证的 GitHub CLI 核对该 SHA 的检查。获准发布后执行：

```sh
./scripts/release.sh "$VERSION" --push
```

脚本检查该 SHA 在 `main` push 上的 `ci-gate`，创建 annotated tag，并原子推送分支和 tag。不加 `--push` 只创建本地 tag。不要移动已发布的 tag；修订产物需新版本。

## 产物与检查

[Release workflow](../../.github/workflows/release.yml) 校验 tag、版本和已通过门禁的提交，再执行嵌入式 Console 构建、lint 和 serving 测试。AMD64 与 ARM64 原生 runner 分别构建并推送平台 digest。AMD64 镜像提供 Linux 归档所用二进制，归档随附 `docs/`、许可证、校验和与发布说明。平台构建和验证完成后，workflow 合成多架构镜像并发布 GitHub Release。公开仓库还会为镜像生成 attestation。

稳定版镜像标记精确版本、`major.minor`、`major` 和 `latest`；预发布版本仅标记精确版本。发布后检查 tag 和实际镜像，再在隔离环境按[生产部署](../user/production-deployment.md)验证服务启动、Console 和日志投影：

```sh
git ls-remote --tags origin "refs/tags/v${VERSION}"
docker run --rm --pull=always "ghcr.io/oai404iao/ai_gateway:${VERSION}" --version
```
