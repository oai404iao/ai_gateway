# 项目级系统 E2E

> 状态：部分实现。第一阶段测试侧覆盖浏览器、Codex HTTP/WS、Pi、共享事件样本与隔离故障注入。
> 普通请求的生产日志准入仍待策略决策与独立实现，不因测试完成而宣称已修复。

## 目标与分层

研究背景见 [Monoize 工程对比](../reference/monoize-engineering-study.md)。
根 `e2e/` 负责跨进程系统验收，根 `mock/` 维护协议场景，不搬迁现有测试：

| 层次 | 入口 | 边界 |
| --- | --- | --- |
| 后端、PG、协议回归 | `tests/` | 快速定位服务端规则和故障 |
| UI 组件/浏览器 | `web/console/src/`、`web/console/e2e/` | MSW 或模拟 Console API |
| 系统验收 | `e2e/run.py` | 真实浏览器、CLI、生产二进制、临时 PostgreSQL |
| 真实服务 | `scripts/run-real-upstream-smoke.sh` | 付费、显式授权 |
| 性能 | `tools/forwarding-perf/` | 独立、手动 opt-in，不由本套件运行 |

## 已实现的场景

`console-route-to-settlement`：

1. 使用真实 bootstrap-admin CLI 初始化空数据库及 migrations。
2. 通过 Console API 准备用户余额、价格、渠道、协议路由和测试 Key，不直接写业务 SQL。
3. 浏览器从 Gateway 嵌入的 SPA 登录，不启动 Vite、不拦截 API。
4. 验证 HttpOnly/Secure/SameSite=Lax refresh cookie；reload 后验证真实 refresh 和轮换。
5. 浏览器带真实 ETag 修改候选 wire model，reload 验证持久化。
6. 用 Playwright HTTP 客户端请求独立数据面，验证修改后的路由生效。
7. 等待真实日志与 `billed_at`，在页面看到请求，再核对费用、余额与 Key 已用额度。

`responses-tool-cycle`：

1. 按 `e2e/clients.json` 校验 Codex 精确版本，隔离 HOME/CODEX_HOME 和工作目录。
2. Responses HTTP/SSE 第一次返回 shell 工具调用，执行 `cat marker.txt`。
3. 第二次请求必须含对应 call ID 和每次运行随机生成、未放入提示词的文件内容。
4. 验证最终输出、恰好两次上游请求、模型改写、终态 usage 和幂等结算结果。

`codex-ws-tool-cycle`：

- 通过真实 Console API 启用系统、用户及渠道 WS，客户端必须完成同连接的两次生成。
- 第二次必须携带上一轮 `previous_response_id` 及工具结果；缺失或换连接均失败。
- 验证最终日志全部使用 `websocket`，任何 HTTP fallback 或额外调用都不能通过。
- `generate:false` warmup 独立计数，允许最多一次、零 usage，不能满足工具覆盖。

`pi-responses-tool-cycle`：

- 固定 Pi 版本、隔离 HOME/配置，关闭启动网络、扩展、skills、上下文文件和项目信任。
- 只启用 read 工具，通过 JSON 事件核验成功工具执行、call ID、随机 marker、
  最终 assistant stop 及 agent_end；同时核验上游收到的工具结果。

输入/输出 usage 为 5/2，每百万价格为 1/2，每次正常生成费用为 `0.000009`；
最终费用按实际已验证的正常请求数计算，warmup 单独核验零 usage/费用。
检查最终日志、账户余额和 Key 额度，不是仅检查 ingress 接收数。
`mock/scenarios/response-events.json` 的 wire 模板由 Python HTTP/WS 和 Rust WS 集成测试消费，
独立负向断言拒绝错误费用、伪造文字、错误 call ID、缺失工具输出、跨连接 continuation。

## 耐久性与故障场景

`db-outage-kill-replay` 暂停本轮 PostgreSQL 容器，确认新请求已写入 checkpoint 后的
spool，再 SIGKILL Gateway；恢复 DB 并用原配置/spool 启动，核验最终日志及一次结算。

`duplicate-replay-no-double-charge` 在 Gateway 停止时只回退私有 checkpoint，模拟 DB 已提交、
本地 checkpoint 未提交的窗口。等待 spool 排空及真实 SQL ingress 计数归零，
核验日志数、余额和额度没有重复变化；不重新 dispatch 模型调用。

`spool-write-enospc` / `spool-write-eacces` 是**风险复现，不是可靠性成功保证**：

- C helper 只通过当前测试 Gateway 的 `LD_PRELOAD` 拦截指定 `events.log` fd 的 `write`，
  返回 ENOSPC/EACCES；不填满宿主磁盘、不改变宿主文件系统权限、不修改生产代码。
- 核验一次写失败的指标；移除注入后再发请求，核验 writer 仍锁定失败、直到进程重启。
- 明确记录普通请求仍 dispatch、终态日志缺失、余额没有对应扣减的现状。
- 重启去除注入后，验证新请求重新正常持久化与结算。

这种 syscall 注入不模拟真实文件系统的部分写、全部 I/O 错误或主机断电。
若以后实施生产 fail-closed，必须同步将风险复现的预期改为拒绝未准入请求，
不能为了维持测试通过而保留危险语义。

Console 使用 `http://localhost` 的 Chromium loopback 安全上下文例外。
这验证真实 cookie 行为，但不验证生产 TLS、反向代理或跨域部署。
Codex 分别显式运行 HTTP 与 WS；没有跨协议转换矩阵，也未覆盖所有客户端/故障组合。

## 本地运行

要求 Linux、C 编译器、Python 3.11+、Docker daemon、OpenSSL、Node、项目固定 Rust/pnpm，
以及 `e2e/clients.json` 指定的 Codex/Pi CLI。数据库镜像按 digest 固定，
WS 依赖按版本与 wheel hash 固定；首次运行可能下载依赖。只使用容器内 psql。

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console exec playwright install --with-deps chromium
pnpm --dir web/console build
cargo build --locked --features embedded-console-ui

# 在自己的工具目录安装固定客户端，不使用个人登录状态
npm install --prefix target/system-e2e-client --ignore-scripts --no-audit --no-fund @openai/codex@0.154.0
npm install --prefix target/system-e2e-client --ignore-scripts --no-audit --no-fund @earendil-works/pi-coding-agent@0.85.1
python3 -m venv target/e2e-venv
target/e2e-venv/bin/pip install --require-hashes -r e2e/requirements.txt
target/e2e-venv/bin/python -m unittest discover -s e2e -p 'test_*.py'
node --test e2e/browser-checks.test.mjs
target/e2e-venv/bin/python e2e/run.py \
  --codex "$PWD/target/system-e2e-client/node_modules/.bin/codex" \
  --pi "$PWD/target/system-e2e-client/node_modules/.bin/pi"
```

PATH 已有匹配版本时可省略两个客户端路径参数。
`--binary` 指向当前 worktree 构建的 embedded-Console 二进制；
debug 模式仍依赖 `web/console/dist`。`--output` 只接受 `target/system-e2e/` 下的新目录。
缺少依赖、版本不匹配、空测试和未执行场景不能算通过。

## 隔离、证据与清理

- 每次独立随机容器、数据库、动态 loopback 端口、临时 JWT/密码/Key 和 spool。
- 不接触 canonical 开发数据库；不读取 `config/`、`.env.real-upstream` 或个人 Codex 状态。
- 客户端只收到本次合成 Key；shell 操作仅限读取私有工作目录的随机 marker。
- CLI 当前在宿主进程运行，不提供容器级文件系统/网络隔离；
  只能连接受控 fixture，不能把本工具直接改接不可信或真实模型。
- PostgreSQL 使用随机密码；只在 loopback 发布端口。清理删除容器及匿名卷。
- 子进程有 deadline 和输出上限，退出/失败/超时/SIGINT/SIGTERM 均进入清理。
  SIGKILL、宿主崩溃无法由进程清理；可按 `ai-gateway.system-e2e=true` label
  检查遗留容器，但不要删除其他仍在运行任务的资源。
- `target/system-e2e/<run>/report.json` 包含场景、版本、源码信息、二进制与 harness 摘要、结构化上游证据、
  结算与清理结果；失败时只保留经过合成凭据替换的日志尾部。
- 原始日志、TOML、JWT、客户端 HOME 不进入报告目录，任务结束后删除。
  不保存 browser storage state、HAR 或含鉴权 Headers 的 trace。
- 退出码：成功 0，场景/基础设施/清理失败 1，中断 130。

新增场景应定义稳定 ID、次数/容量/时间上限、成功 oracle 和至少一个负向测试。
共享协议样本不意味着复用所有服务端实现；性能 Mock 不需要承担工具客户端状态机。

## CI

`reusable-quality.yml` 的 `system-e2e` 在 Rust 或现有浏览器门禁被选中时运行，
构建嵌入式二进制并执行离线测试和完整系统链路，纳入 `quality-gate`/`ci-gate`。
根 `e2e/`、`mock/` 可执行文件变更选择 Rust/Console/文档检查；其 Markdown 只选文档。
PR 不写 Rust cache；系统测试报告保留七天。CI 安装固定客户端，不访问付费模型。

## 第一阶段完成边界

- [x] WS 工具连续交互与禁止意外 fallback。
- [x] 第二客户端 Pi，以及协议/工具失败 oracle。
- [x] Rust/CLI 共享 wire 事件模板，不强制所有 Mock 共用执行器。
- [x] 真实进程 kill/restart、重复重放以及进程局部 ENOSPC/EACCES 注入。
- [ ] 根据[日志准入审查](request-log-admission-review.md)确定生产策略后，独立实现与验证。

本阶段不修改生产转发、schema、计费公式或日志准入语义，也不增加 SQLite。
