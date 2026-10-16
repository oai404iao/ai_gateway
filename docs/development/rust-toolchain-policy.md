# Rust 单工具链策略

> 状态：当前。

开发、CI、安全分析和生产构建都使用 Rust 1.97.1。仓库只维护这一精确工具链，
不声明更低 MSRV，也不把最新版 stable 自动当成受支持版本。

## 本地使用

从仓库根目录运行，rustup 会读取 `rust-toolchain.toml`：

```bash
rustup show active-toolchain
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

如果本地与 CI 不一致，先检查工作目录、rustup override 和显式 `cargo +<toolchain>`，
不要通过修改依赖或改写锁文件掩盖工具链差异。PostgreSQL 测试仍需要数据库，
feature-specific 验证按[CI](continuous-integration.md)和[发布门禁](releasing.md)追加。

## 单一版本

项目只支持并持续验证一个精确的 Rust 工具链版本：

- **开发、CI 和生产构建工具链：Rust 1.97.1。**
  `rust-toolchain.toml` 固定本地默认工具链；普通 CI、Security、Release CI 和
  `Dockerfile` 使用同一版本执行格式化、Clippy、测试、分析和 release 构建。
- 根包与性能工具的 Cargo manifest 不声明更低版本兼容边界。项目不承诺旧版
  编译器可以构建当前源码，也不运行第二套旧版工具链门禁。

`Dockerfile` 中的 `CARGO_CHEF_RUST_VERSION` 只选择提供 `cargo-chef` 二进制的
来源镜像。项目依赖和 release 二进制始终在由 `RUST_VERSION` 选择的官方 Rust
镜像阶段编译，因此这两个参数不要求取值相同。

## CI 与发布门禁

Pull Request 和 `main` push 只运行 Rust 1.97.1 质量 job，要求 `fmt`、
workspace-wide Clippy、完整测试以及 embedded Console feature 路径通过。
Release CI 和 Docker 使用同一版本生成发布二进制，不再重复运行旧编译器兼容 job。

## 升级策略

工具链升级是一项协调变更，不是只改一行版本：

1. 更新 `rust-toolchain.toml`、Docker `RUST_VERSION` 及匹配的镜像 digest，检查普通 CI、
   Security 和 Release 的工具链 pin。所有外部 Action 继续使用完整 commit SHA。
2. 运行完整 Rust、feature、发布和容器门禁，确认依赖解析和实际编译都使用新版本。
3. 同步 README、部署/发布文档、Agent 指令、本策略与 Changelog，记录未运行的检查。

`CARGO_CHEF_RUST_VERSION` 不必跟随项目编译器；只在升级 cargo-chef 来源镜像时连同
其 digest 复核。不能为了让两处字符串相同而引入无关镜像更新。

## 来源

| 内容 | 来源 |
| --- | --- |
| 支持的 Rust 工具链 | `rust-toolchain.toml` |
| Package metadata 与依赖 | `Cargo.toml`、`tools/forwarding-perf/Cargo.toml` |
| 普通 Rust 质量门禁 | `.github/workflows/reusable-quality.yml` |
| CI 路径选择与最终 gate | `.github/workflows/ci.yml` |
| Security 与 Release 构建 | `.github/workflows/security.yml`、`.github/workflows/release.yml` |
| 容器编译工具链 | `Dockerfile` 的 `RUST_VERSION` |
