# 独立计量事实与结算回执

> 状态：当前。

终态事件先成为不可变计量事实，再独立结算和投影到日志。日志可以暂时不可写或尚未可见，
但已验证的费用仍能结算；查询日志不再拥有扣款权。升级切换后不能与旧结算者混跑，
也不能直接回退旧二进制。

## 目录

- [数据所有权](#数据所有权)
- [写入、重放与归属](#写入重放与归属)
- [金额分类与结算资格](#金额分类与结算资格)
- [唯一结算事务](#唯一结算事务)
- [账本迁移与回退](#账本迁移与回退)
- [验证](#验证)

## 数据所有权

| 数据 | 所有者与不变量 |
| --- | --- |
| request_metering_facts | MeteringRepository 从可信终态物化，一 UUID 一行；`upstream_credential_id` 为请求时凭证 UUID 或 NULL。禁止 UPDATE/DELETE，不引用日志 |
| request_settlements | SettlementRepository 的唯一普通扣款认领，一事实一回执；金额/币种与合格事实一致，不可删改 |
| request_settlement_pending | 首次物化合格事实时创建，结算时删除；禁止 UPDATE，无回执不得删除 |
| request_log_ingest | 耐久事件入口；计量/投影独立退避，事实与日志均持久化后才可 ack |
| request_logs | 诊断/展示投影；费用和 usage 是不可变展示副本，不作为财务读源 |

事实不由日志 INSERT 触发器生成。费用查询不回退日志副本。
没有自动补账/冲正接口、日志 TTL 或事实清理任务。
重放水位/幂等墓碑设计完成前，事实和回执都不得清理；只删回执也会重新开放扣款权。
墓碑身份仍保留历史账户归属。

## 写入、重放与归属

```text
intent / terminal slot / spool
  → durable ingress 提交 → spool checkpoint
  → fact（含凭证 ID）+ 新事实 pending + ingress.metered_at 同事务
       ├─ receipt + balance/Key quota + pending 删除
       ├─ 财务统计 / Codex 窗口费用
       └─ 独立日志投影 → ingress ack
```

PG 用 COPY，SQLite 用批量事务 INSERT。checkpoint 等待耐久接收，不等待日志可见。
计量和投影是独立任务：批量计量失败先退避，再逐条隔离。
坏 journal、财务冲突不标 ready；展示字段约束失败只延迟投影，不撤回事实或阻止结算。
正常关闭按 ingest → 计量 → 投影 → 结算尽量排空；超时留待重启。

直接终态/旧内存队列的 insert/insert_batch 先独立提交事实，再尝试日志投影，
后者失败不回滚事实。生产 projector 使用 project_batch，不重复生成事实。
所有入口共用 MeteringRepository 和 SettlementRepository，不保留第二套结算算法。

### 重放比较

- 同 UUID、相同规范化财务内容成功重放；不同内容保留原事实与 ingress，
  标记 financial_replay_conflict，不覆盖、不二次扣款。
- 只有新事实创建 pending。并发结算后重放不能重新创建已完成工作。
- 仅非财务字段不同由日志侧 duplicate_conflict 保留证据，不撤销有效结算。
- 插入和比较使用相同列语义：UUID、微秒时间、12 位价格、8 位费用。时间绑定与事实序列化采用同一微秒精度，避免同一事件因亚微秒舍入差异而误判冲突。
- 不按当前模型价格重算费用。金额与舍入顺序见[行为契约](persistence-contracts.md)。

### 凭证归属

凭证 ID 直接写入不可变事实；`NULL` 表示无凭证，没有旧身份回退或归属侧表。选路快照捕获凭证，而不是在完成或查询时读取当前渠道绑定。归属冲突不覆盖，也不得推进 ingress。
渠道改绑因此不能移动历史或在途请求费用。

## 金额分类与结算资格

amount_state 由字段生成，不由调用方随意填写：

| 状态 | 含义与处理 |
| --- | --- |
| priced | 确定非负费用、模型身份、完整有效价格；仍需 Key 归属正确 |
| zero_by_policy | 明确 failed/cancelled 且费用为 0，可缺 usage/价格，写一次零额回执 |
| unknown | 缺确定费用的其他可信终态；不记零、不伪造 usage、不自动冻结普通用户 |
| not_applicable | 无费用 rejected 或当前不可计价搜索，不是零额已结算 |
| invalid | 有费用却缺模型/价格等合法结算证据；保留并隔离，不阻塞正常事实 |

intent-only 不是可信终态，继续保留本地证据。scheduled probe 不因 source 免费。
数据库同时约束身份、操作/格式、usage 范围、USD、正 token 单位及价格完整性；
“费用非 NULL”并非回执资格，不能只靠 worker 的筛选保护。

成本统计沿用 cost_amount IS NOT NULL 口径，不代表可扣款金额或提供方账单。
个人 usage、费用、排行榜及 Codex 窗口读事实；渠道延迟/成功率读日志投影。
Console billed_at 及其筛选由回执 settled_at LEFT JOIN 派生。
费用/余额早于日志页可见是允许的最终一致性，不是共享数据库 CPU、磁盘或不可用的资源隔离。

reconciliation_counts 分别统计 unknown、invalid 和账户不匹配，
健康采样通过 ai_gateway::metering_health 记录变化，不新增 Console 字段。
正常 settlement backlog 仅包含可自动处理工作；为空不表示未知/异常已经结清。
解码、写入和冲突另有持久化重试原因及 ERROR。

## 唯一结算事务

```text
合格且账户归属正确的 pending
  → 插入此前不存在的 UUID 回执
  → 按本事务实际新增回执聚合用户余额和 Key 已用额度更新
  → 检查更新集合
  → 删除对应 pending
  → COMMIT
  → 更新进程内 soft-quota
```

任一步失败全部回滚。提交成功但回复丢失，重试依据回执返回 AlreadyBilled，不再扣款。
policy_version=1 标识回执资格契约，不重新解释历史定价。

PG 账户按 UUID 排序取得 FOR NO KEY UPDATE，不与事实/日志 INSERT 的 FK KEY SHARE 冲突。
死锁/序列化失败重试整个事务，不能只重放余额 UPDATE。
SQLite 在唯一 BEGIN IMMEDIATE 内使用精确金额运算，最终列溢出同样整批回滚。

恢复按 pending 的 (completed_at,request_id) 索引扫描，而非反复 anti-join 全部历史。
unknown/invalid 使用专门部分索引；账户不匹配留在 pending，但不自动扣款并独立报告。
这是至少一次事件投递下的幂等账户效果，不保证上游恰好调用一次。

## 账本迁移与回退

当前基线不接收旧库或旧 journal，不从旧日志推导余额或回执。新库是独立账本，不是旧库的转换结果。当前账本恢复使用同一停写点的数据库和完整 spool；原账本切回旧版须用旧制品与匹配备份，并核对备份后新增的上游使用。见[切换步骤](../user/upgrades.md)。

## 验证

tests/contracts/facts.rs 的关键契约：

- 日志表被锁或展示非法，费用仍能结算和查询；
- 财务/展示冲突分离，重启无双扣，投影删除后仍能幂等恢复；
- fact/pending/ready 原子性、回执资格/不可变性、提前 ack 拒绝；
- 当前格式 ingress 重复交付、亚微秒时间比较及账本重开；
- 外键共享锁不阻塞结算，已结算历史不扩大 pending 扫描。

另保留[金额/接口契约](persistence-contracts.md)、[SQLite 财务对照](sqlite-metering.md)、
Console spec 和[系统 E2E](system-e2e.md)。仓储测试、真实 CLI/浏览器恢复和发布容量演练
各有边界，不互相替代；转发改动的付费上游回归须按仓库规范获授权后执行。
