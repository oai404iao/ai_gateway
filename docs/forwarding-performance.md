# 转发性能测试设计与使用说明

本文档定义 `ai-gateway` 的手动端到端转发性能测试方案。性能测试不属于普通
单元测试或集成测试，默认不会被 `cargo test`、常规开发检查或 CI 执行。

## 目标

性能 Harness 用于回答以下问题：

1. Gateway 相对于直连 Mock 上游增加了多少延迟。
2. 不同 API 格式、非流式响应和 SSE 流式响应下的可持续吞吐。
3. 固定并发下的 p50/p95/p99 延迟、TTFT、错误率和峰值上游并发。
4. 高转发吞吐下，异步请求日志是否仍能完整持久化。
5. 测试客户端或 Mock 上游是否先于 Gateway 成为瓶颈。

它不是生产容量承诺。第一阶段全部组件默认运行在同一台机器上，CPU、网络和
PostgreSQL 会互相竞争，因此结果主要用于：

- 同一台机器、同一运行参数下的版本对比。
- 定位数据面热点。
- 发现明显性能回退。
- 为后续分布式压测提供基础。

## 总体架构

```text
ai-gateway-perf 编排器
├── 创建随机临时 PostgreSQL 数据库
├── 执行 migrations
├── 写入测试用户、API Key、模型、渠道组、渠道和模型规则
├── 启动独立 Mock LLM 上游进程
├── 生成临时 Gateway TOML
├── 启动 release 版 ai-gateway 进程
├── 客户端直连 Mock，得到测试环境基线
├── 客户端通过 Gateway 请求 Mock
├── 优雅停止 Gateway，等待请求日志队列排空
├── 查询 request_logs 并计算持久化率
├── 输出 JSON、Markdown、场景配置和进程日志
└── 停止 Mock 并删除临时数据库
```

实现位于：

```text
tools/forwarding-perf/
scripts/run-forwarding-perf.sh
```

性能工具是独立的 Cargo workspace package，不会被编译进生产
`ai-gateway` 二进制。

## 为什么必须有直连基线

每个场景执行两次：

```text
性能客户端 ───────────────> Mock
性能客户端 ──> ai-gateway ──> Mock
```

直连结果给出当前机器上“客户端 + Mock + loopback 网络”的能力上限。报告会计算：

- Gateway/直连成功 RPS 比例。
- Gateway p50 延迟增量。
- Gateway p99 延迟增量。

如果直连 Mock 本身已经饱和，则 Gateway 结果不能被解释为 Gateway 的真实上限。

## 数据库隔离

性能工具默认读取：

```text
TEST_DATABASE_ADMIN_URL
```

未设置时使用：

```text
postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres
```

该 URL 必须指向 `postgres` 等管理数据库，不能指向正常的 `ai_gateway`
应用数据库。每次运行会创建：

```text
ai_gateway_perf_<random uuid>
```

数据库创建后会：

1. 执行仓库中的全部 SQLx migration。
2. 写入两个 API 格式对应的渠道组和渠道。
3. 为每个性能场景创建独立模型和模型规则。
4. 创建只用于本次测试的客户端 API Key。
5. 编译一次完整运行时快照，提前发现种子数据与当前 schema 的漂移。

默认在测试结束后执行：

```sql
DROP DATABASE "<generated-name>" WITH (FORCE)
```

仅在显式传入 `--keep-database` 时保留数据库。

## Mock LLM 上游

Mock 是单独的 Axum/Tokio 进程，提供：

```text
GET  /health
POST /v1/chat/completions
POST /v1/responses

POST /__perf/config
POST /__perf/reset
GET  /__perf/stats
```

`/__perf/*` 仅绑定在本机临时端口，不属于产品 API。

支持两类响应：

### JSON

- 可立即返回。
- 可配置固定响应延迟。
- 返回包含 usage 的格式兼容 JSON。

### SSE

- 可配置 TTFT。
- 可配置 chunk 间隔和数量。
- Chat Completions 最终发送 usage frame 和 `[DONE]`。
- Responses 最终发送 `response.completed` 和 usage。

Mock 不保存每个完整请求，只使用原子计数器记录：

- accepted/completed/cancelled。
- 当前和峰值 in-flight。
- 请求与响应字节数。
- 无效 Authorization 数量。

## 高并发客户端

第一阶段实现固定并发的 closed-model 客户端：

```text
每个 worker:
    发送请求
    完整消费 JSON 或 SSE body
    记录结果
    立即发送下一个请求
```

所有 worker 共享一个 `reqwest::Client`，以测试正常的连接池复用。客户端会：

- 完整消费响应 body。
- 校验成功状态。
- 流式请求校验 `text/event-stream`。
- Chat SSE 校验 `[DONE]`。
- Responses SSE 校验 `response.completed`。
- 记录首次非空 body chunk 作为客户端 TTFT。

延迟使用固定内存的近似直方图统计。127 微秒以上的桶相对精度约为 1.6%，不会
因数百万请求而无限增长内存。

## 第一阶段场景

### quick

用于验证 Harness 和短时间开发对比：

| 场景 | API | 上游行为 | 并发 | 预热 | 采样 |
| --- | --- | --- | ---: | ---: | ---: |
| chat-json-fast | Chat Completions | 立即 JSON | 32 | 2s | 5s |
| responses-json-fast | Responses | 立即 JSON | 32 | 2s | 5s |
| chat-sse-short | Chat Completions | 短 SSE | 32 | 2s | 5s |
| responses-sse-short | Responses | 短 SSE | 32 | 2s | 5s |

### standard

用于正式的本地性能对比：

| 场景 | API | 上游行为 | 并发 | 预热 | 采样 |
| --- | --- | --- | ---: | ---: | ---: |
| chat-json-fast | Chat Completions | 立即 JSON | 128 | 10s | 30s |
| responses-json-fast | Responses | 立即 JSON | 128 | 10s | 30s |
| chat-json-50ms | Chat Completions | 延迟 50ms JSON | 256 | 10s | 30s |
| responses-json-50ms | Responses | 延迟 50ms JSON | 256 | 10s | 30s |
| chat-sse-short | Chat Completions | 100ms TTFT，20 chunks | 256 | 10s | 30s |
| responses-sse-short | Responses | 100ms TTFT，20 chunks | 256 | 10s | 30s |

每个场景分别运行直连和 Gateway 两次，因此 `standard` 会持续数分钟。

## 请求日志完整性

Gateway 当前使用有界异步队列持久化请求日志：

- 请求路径调用 `try_send`，不会等待 PostgreSQL。
- 队列满时日志被丢弃。
- 后台使用固定大小的批量插入，并允许两个插入批次并行执行。
- 插入与结算是独立阶段；结算会按用户和 API Key 聚合费用后批量更新。
- 结算通知队列满时只丢弃内存提示，已持久化的未结算行仍由定时恢复扫描处理。

因此只报告 RPS 会掩盖日志丢失。性能工具为每个场景使用独立 `client_model`，
在 Gateway 优雅关闭、日志队列完成排空后统计：

```text
expected_request_logs = warmup requests + measured requests
request_log_persistence_ratio = persisted request_logs / expected_request_logs
```

报告还会列出 succeeded/failed/rejected/cancelled 各类持久化结果。默认生成配置
使用与 `config.example.toml` 相同的 `1024` 请求日志队列容量。

## 手动执行

先启动开发 PostgreSQL：

```bash
docker compose up -d
```

短测试：

```bash
./scripts/run-forwarding-perf.sh --profile quick
```

标准测试：

```bash
./scripts/run-forwarding-perf.sh --profile standard
```

使用其他管理数据库：

```bash
./scripts/run-forwarding-perf.sh \
  --profile quick \
  --database-admin-url \
  postgres://user:password@127.0.0.1:5432/postgres
```

保留临时数据库用于排查：

```bash
./scripts/run-forwarding-perf.sh --profile quick --keep-database
```

脚本会显式构建：

```text
cargo build --release --locked --package ai-gateway
cargo build --release --locked --package ai-gateway-perf
```

性能测试不会由任何普通测试命令自动启动。

性能工具自身的轻量单元测试不会产生并发流量：

```bash
cargo test --package ai-gateway-perf
cargo clippy --package ai-gateway-perf --all-targets
```

如需只验证 migration 和控制面种子数据，不启动 Gateway 或负载客户端：

```bash
cargo test --package ai-gateway-perf \
  temporary_database_seeds_a_compilable_snapshot -- --ignored
```

该命令需要 PostgreSQL，但只创建并立即删除一个临时数据库。

## 产物

默认输出到：

```text
target/perf/reports/<timestamp>-<run-id>/
```

包含：

```text
report.json
report.md
scenario.toml
gateway.log
mock.log
<scenario>-direct.json
<scenario>-direct.log
<scenario>-gateway.json
<scenario>-gateway.log
```

临时 `gateway.runtime.toml` 含数据库连接信息，只在子进程运行期间存在，报告完成
后会删除。

主要指标：

- achieved/success RPS。
- error rate。
- latency p50/p90/p95/p99/p99.9/max。
- SSE TTFT。
- bytes/second。
- Mock peak in-flight。
- Gateway/直连吞吐比和延迟增量。
- 请求日志持久化率。
- Git commit、dirty 状态、Rust 版本、操作系统和逻辑 CPU 数。

## 安全与清理

- 所有服务只监听 `127.0.0.1`。
- 只允许删除 `ai_gateway_perf_` 前缀的数据库。
- Ctrl-C 会停止 Gateway 和 Mock，并清理数据库。
- Gateway 通过 SIGTERM 优雅停止，以便请求日志 worker 排空。
- 子进程启用 kill-on-drop，异常退出时不会故意留下压测进程。
- 正常报告不保存数据库密码或测试 API Key。

## 第一阶段边界

当前已实现：

- 独立手动编排器。
- 全新临时数据库。
- 全新 release Gateway 实例。
- Chat Completions 和 Responses。
- JSON 与 SSE Mock。
- 固定并发客户端。
- 直连基线。
- JSON/Markdown 报告。
- 请求日志持久化率。

后续阶段计划：

1. Open-model 固定 RPS 调度与 scheduler lag。
2. 自动并发阶梯和饱和点识别。
3. Gateway/Mock/客户端 CPU、RSS、FD 采样。
4. 多客户端分片和远程压测。
5. 长时间 SSE soak。
6. 基线文件与自动相对回退阈值。
