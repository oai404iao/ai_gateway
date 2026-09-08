# Rust 单工具链策略

> 状态：当前。

## 单一版本

项目只支持并持续验证一个精确的 Rust 工具链版本：

- **开发、CI 和生产构建工具链：Rust 1.97.1。**
  `rust-toolchain.toml` 固定本地默认工具链；普通 CI、Security、Release CI 和
  `Dockerfile` 使用同一版本执行格式化、Clippy、测试、分析和 release 构建。
- 根包与性能工具的 Cargo manifest 不声明更低版本兼容边界。项目不承诺旧版
  编译器可以构建当前源码，也不运行第二套旧版工具链门禁。

日常直接运行 `cargo` 即会使用 `rust-toolchain.toml`：

```bash
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

`Dockerfile` 中的 `CARGO_CHEF_RUST_VERSION` 只选择提供 `cargo-chef` 二进制的
来源镜像。项目依赖和 release 二进制始终在由 `RUST_VERSION` 选择的官方 Rust
镜像阶段编译，因此这两个参数不要求取值相同。

## CI 与发布门禁

Pull Request 和 `main` push 只运行 Rust 1.97.1 质量 job，要求 `fmt`、
workspace-wide Clippy、完整测试以及 embedded Console feature 路径通过。
Release CI 和 Docker 使用同一版本生成发布二进制，不再重复运行旧编译器兼容 job。

## 升级策略

升级 Rust 时必须作为一项协调变更完成：

- 同步更新 `rust-toolchain.toml`、`Dockerfile` 的 `RUST_VERSION`，以及普通 CI、
  Security 和 Release workflow 中的工具链 pin。
- 运行完整 Rust、feature、发布和容器门禁，确认依赖解析与产物构建都使用新版本。
- 同步更新 README、部署/发布文档、Agent 指令、本策略和 Changelog。
- 不要仅为了匹配项目编译器而修改 `CARGO_CHEF_RUST_VERSION`；只有升级
  `cargo-chef` 来源镜像时才连同其 digest 一起复核。

## 来源

| 内容 | 来源 |
| --- | --- |
| 支持的 Rust 工具链 | `rust-toolchain.toml` |
| Package metadata 与依赖 | `Cargo.toml`、`tools/forwarding-perf/Cargo.toml` |
| 普通 Rust 质量门禁 | `.github/workflows/reusable-quality.yml` |
| CI 路径选择与最终 gate | `.github/workflows/ci.yml` |
| Security 与 Release 构建 | `.github/workflows/security.yml`、`.github/workflows/release.yml` |
| 容器编译工具链 | `Dockerfile` 的 `RUST_VERSION` |
