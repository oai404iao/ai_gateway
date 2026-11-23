# 持久化行为契约

> 状态：当前。

两后端按相同的业务输入验证账户结果，不要求使用相同的 SQL、锁或错误码。表结构分别由 [PostgreSQL](../../migrations/0001_baseline.sql) 和 [SQLite](../../migrations/sqlite/0001_baseline.sql) 的当前基线定义。

## 金额与归属

| 情形 | 必须保持的结果 |
| --- | --- |
| 单价和费用 | 请求开始时选定模型、价格和倍率。有效单价取 12 位小数；各项求和并除以计价单位后，费用取 8 位。均为中点取偶，不能先把分项费用取整。 |
| 余额与 Key 软额度 | 余额可以透支，已用额度不能为负。批量金额运算溢出时整笔回滚。修改 Key 额度上限不清零已用额度。 |
| 金额未知或不适用 | 成功但无可信费用、未计价的拒绝，不能当作已结算零费用。明确失败/取消的请求按零费用策略处理，仍可保留观察到的 usage。 |
| 凭证归属 | `request_metering_facts.upstream_credential_id` 记录请求时的凭证；NULL 表示无凭证。重放比较包含该列，不按渠道现绑定、当前价格或展示表推算。 |
| 身份与管理 | 定时探测按系统身份、实际价格入账。管理员设余额、邀请码的初始余额属于控制面事务，不生成请求扣款回执。Codex 窗口重置也不是普通账户退款。 |

## 提交与恢复

1. 本地 spool checkpoint 只确认数据库已提交的整批事件。入口必须在事实与查询投影都持久化后才可 ack；正常结算不等待查询日志可见。
2. 物化新事实时在同一事务设置 ready 并创建合格 pending。已有 UUID 内容不等价时保留原事实与冲突记录，不覆盖；已结算 UUID 的等价重放不能重新创建 pending。
3. 财务流水线只按新增的唯一回执汇总余额与 Key 额度，在同一事务更新全部账户并移除 pending。回复超时须查回执，不能凭超时重复更新余额。用户与 Key 归属不符的行不结算，不阻塞其他合格行。
4. 保留事务内资格检查、直接 SQL 约束和数值上限。应用层的校验不能代替数据库拒绝非法认领。删除查询日志不能删除事实或回执；未知 intent 不是终态，既不能生成费用，也不能推断零费用。

入口只处理当前终态格式，启动只接受空库或匹配当前基线的库。测试当前格式的重复交付和服务重开，不代表支持旧版账本。新旧账本的切换与停写备份见[升级指南](../user/upgrades.md)。

## 修改后如何验证

从仓库根目录运行；PG helper 需能创建随机测试库的管理连接，不指向业务库：

```bash
cargo test --locked --lib application::billing::tests
cargo test --locked --test control_plane_integration persistence_contracts
cargo test --locked --test control_plane_integration metering_facts
cargo test --locked --workspace --features sqlite-backend
```

- [`persistence.rs`](../../tests/contracts/persistence.rs) 验证金额精度、账户更新不完整或溢出时的整批回滚、受限恢复扫描及数据库拒绝非法金额。用[SQLite 计量对照](sqlite-metering.md)验证文件后端的精确聚合与取消边界。
- [`facts.rs`](../../tests/contracts/facts.rs) 验证 readiness 与事实原子性、回执资格、投影删除后财务记录保留，以及投影锁住或失败时结算仍可推进。
- [系统 E2E](system-e2e.md) 在隔离实例验证 DB outage、slot 重放、kill/restart 和成对恢复后的账户结果。幂等账户效果不证明上游恰好调用一次，进程崩溃测试也不能证明任意设备断电无损。

仓储接口见[持久化操作接口](persistence-interfaces.md)，扣款资格与读源见[独立计量](independent-metering.md)和[统计口径](statistics.md)。
