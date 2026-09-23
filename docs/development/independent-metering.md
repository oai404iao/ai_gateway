# 独立计量事实与结算回执

> 状态：当前。独立事实由 `0063` 引入；`0069` / SQLite `0009` 增加不可变请求凭证归属。
> 这是停机硬切换，不能与旧结算者混跑，也不能直接回退旧二进制。

## 所有权

| 数据 | 所有者与约束 |
| --- | --- |
| `request_metering_facts` | `MeteringRepository` 从可信终态事件物化；一请求 UUID 一行，禁止 UPDATE/DELETE，不引用日志表 |
| `request_credential_attributions` | 与事实同事务持久化所选凭证归属；区分旧未知、明确无认证和凭证 UUID，禁止 UPDATE/DELETE |
| `request_settlements` | `SettlementRepository` 的唯一普通扣款认领；一事实 UUID 一回执，金额/币种必须与合格事实一致，禁止 UPDATE/DELETE |
| `request_settlement_pending` | PG 内部未结算工作集合；首次物化合格事实时同事务创建，结算时同事务删除；无回执不得删除或修改 |
| `request_log_ingest` | COPY 耐久接收，计量/日志分别退避；只有事实与日志均持久化后才能 ack |
| `request_logs` | 诊断/展示投影，保留旧费用/usage 的不可变展示副本，不再拥有结算认领权 |

不在日志表上通过触发器生成事实；费用查询和拼车恢复不回退读取日志副本。
主/次拼车窗口仍由原 WAL 独立维护，不增加第二次普通账户扣款。
没有新后台日志删除任务，也没有自动补账/冲正接口。

## 写入与恢复

```text
intent / terminal slot / spool（第一阶段语义不变）
  -> PG COPY ingress 提交 -> spool checkpoint
  -> MeteringRepository 物化事务
       写不可变事实/凭证归属 + 新事实的待结算工作项 + ingress.metered_at
       ├─ 唯一回执 + 余额/Key 额度 + 删除工作项（单事务）
       ├─ 财务统计 / Codex 窗口成本 / 拼车恢复
       └─ 日志投影 -> 确认 ingress
```

计量阶段与日志投影使用独立任务。计量的批量 SQL 失败先退避，再逐条隔离；
坏 journal 或财务冲突保留原 ingress，不能提前标为可投影。
journal v8 使用选路快照中的凭证身份，不在完成或查询时读取当前渠道绑定。
`request_credential_identities` 只对旧未知归属回退到冻结的渠道身份注册表，
明确无认证不回退。重复归属冲突不覆盖已存记录；侧表写入失败不能提交事实或推进 ingress。
展示状态等非财务约束失败，只延迟日志投影，已确认费用可先结算。
正常停机按 ingest → 计量 → 投影 → 结算顺序尽量排空；超时的耐久记录留待重启。

`RequestLogRepository::insert/insert_batch` 是旧内存队列及直接终态调用的顺序入口：
先独立提交事实，再尝试日志投影；后者失败不回滚事实。生产耐久 projector 直接调用
`project_batch`，不重复写事实。两条路径使用同一 `MeteringRepository` 和 `SettlementRepository`。

同 UUID、相同规范化财务内容可重放；财务内容不同不覆盖、不二次扣款，
ingress 标记 `financial_replay_conflict`。仅诊断不同由日志侧 `duplicate_conflict` 保留证据。
事实的新建、待结算工作项及可投影标记在一个事务中完成。
只有首次新建事实才产生工作项，重放不能在并发结算后重新创建已完成工作。

PG JSON timestamp cast 会舍入亚微秒，而原 SQLx timestamp binding 截断到微秒。
事实序列化显式按后者规范化开始/完成/价格时间，避免升级后旧 journal 的同一事件被误判冲突。
金额仍是 Decimal；12 位有效价格、8 位费用的舍入规则与 P1 不变，不重新计算历史成本。

## 分类与资格

`amount_state` 由数据库字段生成，不能由调用方随意指定：

| 状态 | 含义 |
| --- | --- |
| `priced` | 有确定非负费用、模型身份和完整价格快照；仍需 Key 归属正确才能扣款 |
| `zero_by_policy` | 明确 failed/cancelled 且费用为 0，允许缺价格/usage；写零额回执 |
| `unknown` | 缺确定费用的其他可信终态；不自动记零、不自动冻结普通用户 |
| `not_applicable` | 无费用的 rejected，或当前不可计价的 standalone web search；不是零额已结算 |
| `invalid` | 有费用但缺合法结算需要的模型/价格证据；保留，不阻塞正常费用 |

只有 intent 不构成终态事实，继续按第一阶段保留本地证据。
scheduled probe 不因 source 被免单。拼车只读取 `priced` / `zero_by_policy` 的费用，
不等待普通账户回执；未知/异常仍按既有 WAL uncertain 规则处理。

成本统计保留既有“费用非 NULL”的口径，不等于合法扣款金额或提供方账单；
不得用统计总额代替逐事实结算资格校验。金额与身份、协议格式/操作、usage 范围、
USD 及价格快照完整性同时受 PG 约束保护。

`MeteringRepository::reconciliation_counts` 分别统计 unknown、invalid、账户不匹配。
每个实例在健康采样时通过 `ai_gateway::metering_health` 报告变化，不新增 Console 响应字段。
正常 settlement backlog 只包含可自动处理工作；未知/异常不能假装为已排空。
计量写入/解码/冲突失败另有持久化重试原因和 ERROR 日志。

## 唯一结算事务与有界恢复

结算只为合格事实插入此前不存在的 UUID 回执，按本事务实际新插入的回执聚合账户更新，
检查更新集合，删除对应 pending，最后提交。任一步失败全部回滚。
已提交但回复丢失时，再次调用根据回执返回 AlreadyBilled，不重新扣款。
回执的 `policy_version=1` 标识本次结算资格/回执契约，不用于重新解释历史定价算法。

账户按 UUID 排序取得 `FOR NO KEY UPDATE`，余额/额度修改不与事实/日志 INSERT 的
外键 `KEY SHARE` 冲突。序列化/死锁异常仍由整体重试处理，不能重放单个扣款语句。

恢复按 pending 的 `(completed_at,request_id)` 索引扫描，而不是反复从全部历史事实
anti-join 回执。unknown/invalid 使用专门的部分索引；普通历史增长不会扩张待结算索引。
账户归属异常留在 pending 但不自动扣款，并进入独立诊断。
事实与回执不清理；未设计重放水位/幂等墓碑前禁止删除它们。

Console `billed_at` 和筛选继续存在，但现在是 LEFT JOIN 回执的 `settled_at`，
不是日志物理列。统计/余额可能先于日志页面出现；渠道组延迟/成功率仍来自日志投影。
这解除逻辑依赖，不隔离 PG 的共享 CPU、磁盘、连接池或数据库不可用风险。

## 0063 升级与回退

1. 在与生产规模相当的隔离副本演练，记录回填耗时、锁等待、WAL/磁盘需求；
   小型集成测试不能替代这一发布前容量验收。
2. 停止所有旧版本流量和 worker，保留未知 intent，做数据库与全部 spool/WAL 的一致性备份。
   0063 不改变 journal 版本，可保留支持版本的 spool/ingress；不能丢弃未决证据来排空。
3. 新版本持 migration advisory lock，在既有单事务批内先排他锁定日志/ingress，
   等待已经在途的旧认领事务结束，再复制历史财务列并逐行核对。
   已 billed 行复制原费用/币种/时间回执，**不更新余额或 Key 额度**。
   未 billed 的合格费用进入 pending；缺价格/费用保留分类。
   已 billed 却不满足新回执资格（例如 Key 归属异常）使整个批次回滚。
4. 删除日志物理 `billed_at`，旧认领 SQL 失败。ingress 删除触发器同时拒绝无计量标记、
   无事实或无投影的 ack，防止旧 projector 提前删除财务证据；这不是允许混跑的保证。
5. 使用新版本、原目录、原业务数据库恢复，验证回执、余额/额度增量、积压及日志/拼车恢复后接流量。

迁移失败可留在旧 schema；迁移提交后优先 forward-fix。
必须恢复备份时停写并恢复同一时点数据库和所有 spool/WAL，核对该时点以后的上游使用；
不能仅回滚数据库/二进制。已经在升级前被手工删除的旧日志无法由 0063 恢复财务证据。
不改旧 migration、不增加新的提交屏障或 `-- no-transaction`。

## 验证

`tests/contracts/facts.rs` 覆盖：

- 日志表被锁、展示字段非法时仍结算并提供拼车/统计费用；
- 财务冲突与展示冲突分别保留，重启后不二次扣款；
- fact/work item/ready 原子回滚与恢复，旧提前 ack 被拒绝；
- eligibility、不可变事实/回执、删除展示投影后重放；
- 回填不动账户、亚微秒历史时间、旧 ingress 重复重放；
- 迁移等待在途旧认领提交后再复制回执，不把已扣款误当作新 pending；
- 错误历史回执使 0063 schema 与 migration 记录一起回滚；
- 外键共享锁下结算继续，以及 2,000 条已结算历史后仅扫描一个新工作项的执行计划。

另运行 P1/P2 契约、完整 PG/Console、系统 E2E 与经授权的真实上游 smoke。
P4 的[成对备份恢复演练](persistence-rehearsal.md)使用部署者确认的小数据量假设，
验证 PG dump/restore 与 spool/拼车 WAL 整体恢复；不称作性能压测或生产容量认证。
真实部署的数据量、环境和停机预算不相符时仍须重新演练。
