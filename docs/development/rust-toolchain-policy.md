# Rust 工具链与 MSRV 策略

> 状态：当前。

## 两个版本

项目明确区分默认构建工具链与最低支持版本：

- **默认开发和生产构建工具链：Rust 1.97.1。**
  `rust-toolchain.toml` 固定本地默认工具链；普通 CI、Release CI 和
  `Dockerfile` 使用同一版本执行格式化、Clippy、测试和 release 构建。
- **最低支持 Rust 版本（MSRV）：Rust 1.92.0。**
  根包与性能工具的 `Cargo.toml` 均以 `rust-version = "1.92"` 声明该边界；
  独立 CI job 使用精确的 1.92.0 工具链执行 workspace 编译和完整测试。

日常 `cargo` 命令自动使用 `rust-toolchain.toml`。验证最低版本时必须显式指定：

```bash
cargo +1.92.0 check --locked --workspace --all-targets
cargo +1.92.0 test --locked --workspace
```

## CI 与发布门禁

Pull Request 和 `main` push 同时要求：

1. Rust 1.97.1 下的 `fmt`、workspace-wide Clippy 和完整测试通过。
2. Rust 1.92.0 下的 workspace `check` 和完整测试通过。

Release CI 和 Docker 使用 1.97.1 生成发布二进制。MSRV 表示源码与依赖可以由
1.92.0 编译和测试，不表示发布二进制包含或运行一个 Rust 工具链。

## 兼容窗口

MSRV 采用约 **N-5** 个稳定版本、约半年的兼容窗口，而不是长期固定：

- 常规依赖维护时检查当前稳定 Rust 与 MSRV 的间隔。
- 默认开发工具链可独立升级到新的稳定 patch/minor，并继续精确固定版本。
- 当间隔明显超过约五个稳定版本或维护成本要求提升时，单独评估并提升 MSRV。
- 提升 MSRV 时同步更新两个 Cargo manifest、CI、README、Docker/发布说明和本
  文档，并在新旧门禁都通过后合并。
- 若 MSRV 提升改变已发布版本的源码构建要求，在 Changelog 和发布说明中明确记录。

## 来源

| 内容 | 来源 |
| --- | --- |
| 默认开发工具链 | `rust-toolchain.toml` |
| 源码 MSRV | `Cargo.toml`、`tools/forwarding-perf/Cargo.toml` |
| 普通、MSRV 与 Release 共用门禁 | `.github/workflows/reusable-quality.yml` |
| CI 路径选择与最终 gate | `.github/workflows/ci.yml` |
| Release 构建与发布 | `.github/workflows/release.yml` |
| 容器构建工具链 | `Dockerfile` |
