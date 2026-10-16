# 设置开发环境

> 状态：当前。面向修改和验证仓库的维护者。

本页帮助你准备源码、数据库和检查命令。只想运行网关，请使用[快速上手](../user/quickstart.md)
或[生产部署](../user/production-deployment.md)，不必安装全部开发工具。

## 准备工具

| 工具 | 用途与约束 |
| --- | --- |
| Rust 1.97.1 | `rust-toolchain.toml` 固定开发、CI 和发行使用的版本；Rust 2024 edition |
| Docker Compose / PostgreSQL | PostgreSQL 集成测试与默认本地数据库 |
| OpenSSL | 本地数据库密码及 Console Ed25519 密钥 |
| Node.js 24、pnpm 11.17.0 | Console 开发、构建、组件与浏览器测试；生产不需要 Node 常驻 |
| Python 3.11+、C 编译器、Linux | 系统 E2E 及原生插件测试等专题；详见各自指南 |

```bash
git clone https://github.com/oai404iao/ai_gateway.git
cd ai_gateway
rustc --version
cargo --version
```

修改前按[项目工作流](../../.agents/skills/ai-gateway-development-workflow/SKILL.md)创建独立
worktree，不直接在主 checkout 的 `main` 开发。已有用户改动不得重置或覆盖。
下面源码相关命令在任务 worktree 根目录运行。

## 准备本地配置与数据库

在全新的配置目录执行；已有文件时先检查，不要重新生成并覆盖密码或 JWT 密钥：

```bash
umask 077
mkdir -p ./config ./data/plugins
openssl rand -hex 32 > ./config/postgres-password
chmod 600 ./config/postgres-password
cp config.example.toml ./config/config.toml
docker compose up -d
```

启动前将 `config/config.toml` 的 `[plugins].directory` 改为刚创建的 `data/plugins` 的绝对路径。
模板默认 `/var/lib/ai-gateway/plugins` 需要预先授予运行用户权限，普通开发用户不能直接使用。

`config/` 中的运行配置和密钥不提交。网关默认读取当前工作目录的 `./config/config.toml`，
也接受显式 TOML 路径；不读取 `.env`，不使用 XDG 配置发现。
启动时应用 migration，控制面初始为空。

若主 checkout 已运行开发 PostgreSQL，复用该实例，而不是从不同 worktree 启动争用固定端口的
Compose 项目。数据库启动位置和隔离要求见[worktree 约定](../../.agents/skills/ai-gateway-development-workflow/references/worktrees.md)。
新增 migration 的任务必须使用独立数据库，不能修改其他任务依赖的共享 schema。

```bash
cargo check --locked
cargo run
```

另一个终端运行 `curl -i http://127.0.0.1:3000/health`，应返回 `204 No Content`。
这只验证存活，不代表路由已就绪。启用 Console、创建管理员和首条路由的完整步骤见
[快速上手](../user/quickstart.md)。

## 找到修改位置

| 目录或文件 | 职责 |
| --- | --- |
| `src/http/` | 公共 API 与独立 Console 的路由、认证中间件和错误映射 |
| `src/application/` | 转发、控制面、Console、日志的业务编排 |
| `src/domain/`、`src/routing/` | 操作/格式、路由实体、选择和健康 |
| `src/runtime_config/` | TOML 与不可变运行时快照 |
| `src/persistence/`、`migrations/` | 仓储、数据库生命周期与 schema |
| `src/upstream/`、`src/transforms/` | 复用传输客户端与受限变换 |
| `src/workers/` | 重载、spool 入口、投影和结算 |
| `crates/connector-sdk/` | 原生连接器 ABI、命令和示例 |
| `web/console/` | React SPA、生成 API 类型、组件和浏览器测试 |
| `tests/`、[`e2e/`](../../e2e/README.md)、[`mock/`](../../mock/README.md) | 集成测试、系统验收与共享协议场景 |
| `tools/forwarding-perf/` | 不链接到生产二进制的手动性能工具 |

阅读[架构](architecture.md)理解边界，再从[专题索引](README.md)选择相关契约。

## 选择验证层级

以下是常用入口；涉及多个区域时合并对应检查，完整要求见
[验证矩阵](../../.agents/skills/ai-gateway-development-workflow/references/verification-matrix.md)。

### 文档

```bash
git diff --check
python3 scripts/check-docs.py
```

### Rust 与双后端

```bash
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

默认构建为 PostgreSQL。涉及 SQLite 时还需双后端回归和真实系统验收：

```bash
cargo clippy --locked --workspace --all-targets --features sqlite-backend
cargo test --locked --workspace --features sqlite-backend
```

系统 E2E 的 `--backend sqlite`、配套构建与隔离条件见[系统 E2E](system-e2e.md)，
不能仅用单元测试替代恢复、lease 和财务验收。
涉及 Codex 的测试须先按[插件测试说明](connector-plugins.md)准备实际动态库，不能以内置替身替代。

### Console 与 API 契约

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console typecheck
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build
```

修改 Console API 时，先改 [OpenAPI](../openapi/console-v1.yaml)，再运行：

```bash
pnpm --dir web/console generate:api
pnpm --dir web/console generate:api:check
cargo test --locked --test console_spec_integration
```

生成声明不能手改。浏览器流程、嵌入式资源和 API mock 的选择见[前端指南](../../web/console/README.md)。

### 外部调用与负载

修改转发路径还需[真实上游 smoke](real-upstream-smoke.md)：必须明确授权并使用专用凭据，
它会产生费用。缺少授权或凭据时如实报告阻塞，不能声称完整验证。

[性能 Harness](forwarding-performance.md)只在明确要求性能运行时执行。构建工具或运行
`cargo test --locked --package ai-gateway-perf` 不会启动负载，也不等于做过性能测量。

## 提交与交付

检查 diff、只包含本任务文件，记录实际运行的命令及结果。推送、创建 PR、合并和发布均按
[项目工作流](../../.agents/skills/ai-gateway-development-workflow/SKILL.md)的授权边界执行。
发布另见[版本发布](releasing.md)，不要把一般开发验证当成发行门禁。
