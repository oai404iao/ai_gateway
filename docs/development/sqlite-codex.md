# SQLite Codex 与拼车

> 状态：当前 S5 实现，验收通过；S6 已接通 Linux `sqlite-backend` 部署。

范围与分阶段门禁见 [SQLite 双后端实施](sqlite-backend.md)。本切片接通全部 Codex
仓储和拼车所有权；S6 的 serve/CLI、备份恢复与发行构建见[部署指南](../user/sqlite.md)。

## 仓储与配额

`src/persistence/sqlite/codex.rs` 及其 `io` 子模块实现凭证导入/去重/更新/批量/软删除、
成对投影、OAuth flow 所有权与一次性消费、token 更新、额度观测/重置/历史、
管理员导出和本人可见列表。沿用既有公共 DTO、权限、ETag 和 prepared-change
编译/审计/提交边界，不添加 Console 契约。

逻辑凭证保持一个 pool 和两个格式投影，Images 默认不启用；两投影共享 token、额度、
代理和 generation。业务 workspace + member 去重、旧 email 大小写匹配、导出选择完整性
与代理秘密可选导出保持 PG 行为。审计不保存 token 或代理密码。
OAuth flow 仅由所属管理员读取，在导入同一事务内消费；回滚不消费 flow。

额度按既有 90 秒窗口身份容差、15 分钟手动重置匹配及零使用窗口漂移规则归并，
旧观测不能覆盖新状态。历史区分 natural/manual/openai_official，保留每窗口独立分页。
本人查询在 SQL 中绑定活跃用户及用户组可见性，不能用客户端参数扩大权限。

窗口费用扫描两投影的不可变 `request_metering_facts`，使用 S4 精确 `CostSum`，
不依赖查询日志或账户结算回执、不转浮点、不按 TEXT 排序或调用 SQLite 金额 `SUM`。
读取历史、费用和凭证视图使用同一个读快照。

## 不跨 HTTP 持有写事务

公共 `CodexRefresh` / `CodexQuotaReset` 是不透明操作对象：

1. SQLite 取得每逻辑凭证的进程内互斥锁，短事务读取记录并检查未决 intent。
   此时仅登记内存所有权；禁用凭证、过期 generation、无效代理或请求构建失败不会写 intent。
2. 应用完成本地校验和完整 provider request 构建，调用 `prepare_dispatch()`。
   短写事务按 generation、updated_at 和未删除状态 CAS，持久化随机 attempt UUID，
   commit 成功后才可发送 HTTP。此后不持有 SQL 事务；其他凭证和普通写入仍可前进。
3. 已确认刷新结果在同一个短事务内验证 attempt/generation、删除 intent、更新 token
   与 generation。已确认兑换同样原子写入重置事件及审计、删除 intent。
4. 取消、超时、未知结果、提交错误和审计错误均保留未决证据；不因锁释放或重启而重试。
   刷新错误状态与仍未解决的 intent 原子保存。`unchanged()` 只适用于 dispatch 之前。

`0003_codex_operations.sql` 新增 SQLite 私有 intent 表及凭证/成对渠道更新保护触发器。
既有 S2 migration 不改写；此表不计入 PG/SQLite 业务 schema 对照清单。
intent 没有自动过期或“重启清零”。它不包含 token、请求正文或 provider 响应。
PG 分支继续使用既有行锁事务，`prepare_dispatch()` 为 no-op，不改 PG 锁语义。

### 未知结果的恢复边界

放弃的 **refresh** intent 只能通过显式重新导入凭据，在已有导入/审计事务内替换 token
并清除；活跃操作不能被重新导入抢占，导入回滚会恢复 intent。
未知 **quota_reset** 不允许以重新导入、等待超时或重启清除，也不伪造已兑换/已退款结果。
当前没有人工确认兑换结果的 API；遇到这种不确定状态会持续拒绝该凭证的受保护操作，
保留数据库和 provider 证据，不能直接删行后重试兑换。S6 仍保留此边界，
不新增人工确认兑换的 API，也不允许自动恢复或绕过保护。

进程互斥锁不代替数据库 CAS 或文件独占。操作对象与拼车账本 lease 都持有 S2
逻辑 `DatabaseOwner`，因此数据库 close 等待它们释放；不能在活跃上游调用期间
关闭/重开库来创建第二组锁。关闭开始后的新数据库访问仍失败，未决 intent 保留。

## 拼车账本与恢复

`SharingLedgerLease` 把 PG 专属连接隐藏在仓储边界内。PG 保持 advisory lock；
SQLite 在 S2 文件/进程所有权之上取得一个数据库实例共享的进程内 lease，
短事务保存/核对唯一 ledger UUID，不跨后台保活持有唯一写连接。
认领时核对持久化 UUID，应用写路径不会更改它；`ping()` 只检查仍持有的数据库文件身份
及未关闭状态，不借用读写连接，避免连接池拥塞被误判为所有权丢失。
同库第二个 lease 或不同账本 UUID 均拒绝。

`SaveCodexSharing`、管理员/本人列表和完整运行时配置保持固定席位、不可变绑定、
身份别名保护、成对投影授权和 8 位金额。审计金额使用与 PG `numeric::text`
一致的字符串，不通过 JSON 浮点往返。

独立 WAL 仍使用[既有拼车协议](codex-sharing.md)，不创建第二套金额状态机。
重启恢复相同 ledger UUID、窗口 epoch、使用量和 uncertain pending；从 S4 不可变事实
查询已知费用进行 UUID 幂等对账。没有日志/账户回执也可对账，未知费用继续冻结，
不得在刷新、重导凭证、换座或重启时补发金额。

## 验证入口

```bash
cargo test --locked --features sqlite-backend --test control_plane_integration sqlite_s5_parity
cargo test --locked --features sqlite-backend --lib application::codex::sqlite_tests
cargo test --locked --workspace --features sqlite-backend
cargo clippy --locked --workspace --all-targets --features sqlite-backend
```

共享 case 在真实 PostgreSQL 新库和 SQLite 文件库上执行，覆盖 CRUD/去重/导出、
OAuth 一次性提交、配额窗口、双投影费用/本人授权、批量回滚、固定席位、
ledger 排他以及 WAL 重启与事实对账。SQLite 专属测试覆盖本地预检无副作用、
generation/version CAS、其他写者前进、同凭证串行、取消/重开、显式重新授权、
审计失败原子回滚和 guard 保持文件所有权。

应用级本地 provider 测试确认 HTTP 到达前 intent 已提交、无效 Header 不写 intent、
取消兑换后不再 dispatch。普通 Rust gate 保留既有 PG、转发、WebSocket 和 WAL 故障测试；
按[真实上游说明](real-upstream-smoke.md)另行执行已授权付费回归。
这些检查与 S6 系统端到端、停机成对备份恢复共同覆盖部署验收；仍须遵守
[原生 SQLite 版本门槛](sqlite-lifecycle.md#原生-sqlite-版本门槛)。

### 本轮验收记录

2026-09-19：默认与 `sqlite-backend` 的完整 workspace 测试通过，
S5 定向测试含 8 个双后端场景、5 个 SQLite 故障场景及一个应用级本地 provider 场景。
应用测试放在 `tests/contracts/sqlite_codex_application.rs`，作为 Codex 模块的单元契约编译，
原始 SQL 仅用于测试故障/证据检查，不进入应用源码边界。

2026-09-19 首次授权执行 `scripts/run-real-upstream-smoke.sh`：Chat 非流式/流式两项通过，
其余六项失败。Responses 三项收到 `503 codex_credential_missing`，
Images 两项收到 `503 codex_credential_draining`，Search 收到 503。
当时按用户选择保留实现并记录阻塞，未修改测试凭据。

2026-09-20 经用户再次授权，使用同一份本地配置重跑完整脚本，8 项全部通过：
Chat 非流式/流式、Responses 非流式边界/流式/WebSocket、Images 生成/编辑及 Search。
Responses 非流式按 Codex profile 验证预期的 `400 codex_streaming_required`，
不是声称 Codex 支持非流式生成。此前上游 503 验收阻塞已解除；后续 S6 已接通部署路径。
