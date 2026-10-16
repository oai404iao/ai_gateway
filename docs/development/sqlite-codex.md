# SQLite Codex 仓储与维护操作

> 状态：当前。Linux sqlite-backend 使用短事务与耐久 intent 处理提供方维护，
> 不跨 HTTP 持有全局写锁。

Codex 凭证、Token 与 quota 是独立状态，导入不创建渠道、能力或路由。
本页重点是“请求可能已经影响提供方，但本地还不知道结果”时如何保留证据并阻止重复兑换。
它不提供人工确认未知兑换的 API。

## 目录

- [凭证、额度与费用](#凭证额度与费用)
- [不跨 HTTP 持有写事务](#不跨-http-持有写事务)
- [未知结果的恢复边界](#未知结果的恢复边界)
- [所有权与关闭](#所有权与关闭)
- [验证](#验证)

## 凭证、额度与费用

sqlite/codex.rs 及 io、operation 子模块实现导入/去重/更新/批量/软删除、
OAuth flow 所有权与一次性消费、Token 更新、quota 观测/重置/历史、
管理员导出和本人可见列表。沿用公共 DTO、ETag、权限及 prepared-change
编译/审计/提交边界，不增加 Console 契约。

当前身份不属于 pool，渠道通过显式 credential_id 绑定。
维护代理与账号模型目录属于 OAuth 扩展；重新导入、启停和改名不重写渠道接入、
能力目录或开关。删除前须解除全部未删除渠道引用。
账号去重与旧 email 大小写匹配保持现有规则；
导出完整性和可选代理秘密导出保持 PG 行为，审计不保存 Token 或代理密码。

OAuth flow 仅所属管理员可读，在导入事务内消费；回滚不能提前消费。
quota 观测沿用 90 秒窗口身份容差、15 分钟手动 reset 匹配及零使用窗口漂移规则，
旧观测不覆盖新状态，历史区分 natural/manual/openai_official，窗口独立分页。
本人查询绑定活动用户及用户组授权的渠道组可见性，并按账号去重，
不能由客户端参数扩大权限。

费用读取不可变 request_metering_facts JOIN request_credential_identities，
按请求发生时凭证归属，不按当前渠道绑定或旧“两个投影”推断。
只有旧未知归属回退冻结注册表，明确无认证不回退。
窗口按 started_at 闭开边界、cost 非 NULL，使用精确 CostSum，
不读日志/回执、不转浮点、不按金额 TEXT 排序或 SUM。
凭证视图、历史与费用在同一读快照中读取。

## 不跨 HTTP 持有写事务

公共 CodexRefresh/CodexQuotaReset 是不透明对象：

1. 取得每凭证的进程互斥，短事务读取并检查未决 intent。
   此时只有内存所有权；禁用、旧 generation、无效代理或请求构建失败都不写 intent。
2. 应用完成本地校验并完整构建 provider request 后调用 prepare_dispatch()。
   短写事务按 generation、updated_at、未删除状态 CAS，写随机 attempt UUID；
   **提交成功才允许发送 HTTP**。
3. HTTP 期间无 SQL 写事务，其他凭证及普通写入可以前进。
4. 已确认 refresh 在短事务验证 attempt/generation、更新 Token/generation 并删除 intent。
   已确认 reset 同事务写 reset 事件、审计及 intent 删除。
5. 取消、超时、未知结果、提交或审计错误保留未决证据。
   refresh 错误状态与未解决 intent 原子保存；unchanged() 仅适用于 dispatch 前。

0003_codex_operations.sql 引入私有 intent 表与更新 fence，
后续拓扑迁移重新安装相应保护，不能修改旧 migration。
intent 不含 Token、请求正文或 provider 响应，不自动过期，也不随重启清零。
它不属于 PG/SQLite 业务 schema inventory 的同形对象。

PG 分支保留跨 HTTP 的行锁事务，prepare_dispatch 是 no-op；
这里没有顺便改变 PG 维护语义，也没有跨提供方/数据库原子提交。

## 未知结果的恢复边界

| intent | 允许的恢复 |
| --- | --- |
| refresh | 仅显式重新导入凭证，在原导入/审计事务中替换 Token 并清除；活跃操作不可被抢占，导入回滚恢复 intent |
| quota_reset | 不允许重新导入、超时、重启或删行清除；持续拒绝受保护操作，保留数据库与提供方证据 |

未知 reset 不能伪造“已兑换”或“已退款”，当前没有人工确认 API，
也不能自动重试可能已经消费 credit 的 HTTP。
备份/恢复必须保留 intent，刷新页面或重开库不是财务重置。
拓扑迁移可能暂时撤 fence 回填元数据，但不得丢 intent，提交前必须恢复保护。

## 所有权与关闭

进程互斥不代替 CAS 或文件独占。guard 持有逻辑 DatabaseOwner，
数据库 close 等待其释放，不能在上游操作活跃时关库/重开创建第二组锁。
关闭开始后新访问失败，未决 intent 仍在。
跨进程 lease 继续保留到进程退出，详见[生命周期](sqlite-lifecycle.md)。

serve、管理员 CLI 与离线成对备份已接入部署，
不再是待开放的 S5 开发接口。恢复步骤见[SQLite 用户指南](../user/sqlite.md)。

## 验证

```bash
cargo test --locked --features sqlite-backend --test codex_capability_integration
cargo test --locked --features sqlite-backend --lib application::codex::sqlite_tests
cargo test --locked --workspace --features sqlite-backend
cargo clippy --locked --workspace --all-targets --features sqlite-backend
```

同一组 case 在真实 PG 新库及 SQLite 文件库运行，覆盖导入/去重/导出、
OAuth 一次性提交、额度窗口、费用归属/本人授权和批量回滚。
SQLite 专属故障覆盖本地预检无副作用、CAS、同凭证串行/其他 writer 前进、
取消/重开、显式重授权、审计回滚和 guard 保持 owner。
应用级本地 provider 测试确认 HTTP 到达前 intent 已提交，
无效 Header 不写 intent，取消兑换后不再派发。

tests/contracts/sqlite_codex_application.rs 作为 Codex 模块单元契约编译，
故障注入 SQL 只在测试，不进入应用边界。
这些测试与[系统 E2E](system-e2e.md)及成对恢复互补，
不能证明设备断电或真实提供方每种不确定结果。

历史验收中，2026-09-19 上游曾返回 missing/draining 等 503；
2026-09-20 重新获授权后八项 smoke 通过。验收范围是当时环境与配置，不是
当前上游可用性保证。Codex 非流式 Responses 测试验证预期 streaming_required 拒绝，
并非支持非流式生成。后续付费回归须重新遵守[上游测试规范](real-upstream-smoke.md)。

## 相关文档

- [双后端实现](sqlite-backend.md)、[计量与结算](sqlite-metering.md)
- [上游身份与能力](upstream-identity-capabilities.md)
- [原生版本门槛](sqlite-lifecycle.md#原生-sqlite-版本门槛)
