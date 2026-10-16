# 持久化边界与独立计量事实

> 状态：当前设计记录。解释已经落地的边界与决策；当前接口、schema 和后端行为以代码、
> [操作接口](persistence-interfaces.md)及[独立计量事实](independent-metering.md)为准。

持久化层隐藏驱动机制，同时保留金额、原子性、幂等和恢复契约。最重要的分离不是拆文件，
而是让财务事实和结算不再依赖 Console 查询日志能否成功投影。PostgreSQL 保留原生优化，
Linux SQLite 实现相同业务不变量，不模拟 PG 方言。

## 目录

- [边界与非目标](#边界与非目标)
- [控制面与外部副作用](#控制面与外部副作用)
- [财务事实的所有权](#财务事实的所有权)
- [唯一结算权](#唯一结算权)
- [流水线与重放](#流水线与重放)
- [迁移与回退](#迁移与回退)
- [验证与设计背景](#验证与设计背景)

## 边界与非目标

| 边界 | 提供什么 | 不提供什么 |
| --- | --- | --- |
| 控制面 | 一致读取、准备指定变更、版本/权限检查、审计 | 任意 executor、快照发布、HTTP 状态码 |
| 身份与 Connector 状态 | 会话、OAuth 和配额的专属操作 | 通用表级 CRUD 或任意连接 |
| 终态接收与日志写入 | 批量耐久接收、重试、投影、确认 | 账户扣款、Console 筛选 |
| 计量 | 不可变事实、费用读取、异常分类 | 查询日志生命周期 |
| 结算 | 唯一回执、账户变更、pending 恢复 | 先公开 claim 再让调用方扣款 |
| 查询 | 受权限约束的日志及统计 | 删除财务证据或持有结算权 |
| 健康 | 不获取连接的池观测与存储积压 | 向应用泄露底层 pool |

这些是窄操作接口，不是通用 Repository/ORM/UoW 框架。
多个接口可共享原连接池；PG 保留控制面与日志池，SQLite 共享单 writer 和 owner。

应用/HTTP 不接触 `PgPool`、驱动事务、SQLx error、SQLSTATE 或约束名。
PG 的 COPY、UNNEST、行锁、advisory lock、行映射及索引留在实现内；
组合根、migration 和后端专属测试可使用驱动。
公开 DTO 只移动实际跨边界部分，不把所有数据库行类型变成领域对象。

本设计不新增收费策略、自动补账/冲正、日志 TTL、消息系统、分布式发布协议或
端到端 exactly-once，也不扩大普通请求准入范围。SQLite 的部署条件由
[双后端实现](sqlite-backend.md)定义，而非“接口可替换”自然推导。

## 控制面与外部副作用

### 候选校验后提交

```text
coordinator 持有进程内 serial 门
  → store.prepare_*：事务内验证身份/版本并执行指定变更
  → 读取完整候选 runtime records
  → 应用编译完整快照并验证上游策略
  → prepared.commit：同事务写既定审计并提交
  → 应用发布完整快照
```

prepared 对象只支持本操作的候选读取、提交和放弃，不暴露通用 begin、executor 或追加修改。
管理员、批量、自助 Key、目录同步、用户设置、自动禁用/恢复和 reload 都遵守同一发布边界。
编译或审计失败必须回滚，不留下成功审计或新快照；原本不记录审计的用户设置不凭重构新增审计。

数据库提交与 ArcSwap 发布并非一个事务。提交后退出由启动/重载恢复；
多个实例通过重载收敛，不宣称原子同步更新。

### Codex 与不确定结果

PG 的 refresh/reset guard 保留行锁覆盖外部 HTTP 的既有语义，应用仍发起网络调用。
不能因接口重构自动重试 Token 轮换或额度兑换，也不能把网络调用搬进仓储。

SQLite 不能在全局写锁下等待 HTTP，使用每凭证互斥、generation/version CAS 和
派发前耐久 intent fencing。未知 refresh 只有显式重导入能清除；
未知 quota-reset 不能靠重导入、超时、重启或删行恢复。具体协议和 owner 生命周期见
[SQLite Codex](sqlite-codex.md)。这是后端专属实现，不表示 PG 已获得相同的 intent 机制。

### 错误不等于重试许可

驱动错误在持久化内部翻译为稳定语义，HTTP 不再解释 SQLSTATE。
不能将全部 unique/FK 都当成同一个冲突，也不能把提交结果未知误报为明确回滚。
有 UUID 的计量/结算按已有事实或回执核对；没有幂等键的控制面创建、
外部 OAuth 和兑换不能盲重试。当前错误类别见[操作接口](persistence-interfaces.md)。

## 财务事实的所有权

| 实体 | 契约 |
| --- | --- |
| `request_metering_facts` | 可信终态 UUID 一行，保存请求/账户身份、操作、选中目标、usage、有效价格、费用和分类；不可变 |
| `request_credential_attributions` | 与事实同事务保存请求时凭证、明确无认证或旧未知；不可变 |
| `request_settlements` | 每个已结算 UUID 一回执，引用事实并记录金额/币种/时间/策略版本；不可变 |
| `request_settlement_pending` | 首次物化合格事实时创建、结算时删除的工作集合，不是新的财务真相 |
| `request_logs` | 可查询展示副本，不拥有结算权，不作为财务读源 |

事实和回执不得外键依赖日志。软删除身份仍须可追溯，不因非 active 状态消失。
事实不复制 prompt、响应正文、Header、秘密或展示错误详情。其维度足够完成财务统计，
不能再靠 JOIN 日志恢复金额或费用归属。

### 费用资格与处理进度分离

| 费用状态 | 条件与结算 |
| --- | --- |
| priced | 确定非负费用、模型及完整有效价格快照；账户归属正确才可结算 |
| zero_by_policy | 明确 failed/cancelled 且费用为 0；可缺价格/usage，写一次零额回执 |
| unknown | 成功缺费用或证据不足；待核对，不伪造 usage、不记零、不自动冻结普通用户 |
| not_applicable | 无费用 rejected 或当前不可计价搜索；不是欠费，也不是零额已结算 |
| invalid | 有费用但缺合法模型/价格证据；保留异常，不阻塞正常事实 |

scheduled probe 不因 source 自动免单。只有 intent 不是可信终态，不能造零费用事实；
intent 也不能证明已派发。普通 backlog、费用未知、账户异常、事件冲突和本地未决证据分别诊断。

数据库必须独立拒绝不合格回执，而非仅依赖 worker 的 WHERE：
费用非负；priced 的模型、USD、正 token 计价单位、生效时间和四项有效单价完整；
zero_by_policy 的 outcome/金额一致；回执 UUID、金额及存在时的币种与事实一致；
Key 属于原 user。历史未 billed 且缺价格的行保留 invalid；
历史已 billed 却不符合资格的行让迁移失败，不能补当前价格。

### 金额契约

- 使用 Decimal：有效价格 12 位，费用/余额/Key 已用额度 8 位，普通金额范围为
  `numeric(24,8)`。中点取偶，golden tests 固定其顺序。
- 先计算并量化有效价格，再按 token 单位对各项求和得到最终费用；
  不分项提前舍入，不改成浮点或另一个计费单位。
- 重放和迁移复制当时的有效快照及费用，不按现价重算。
- 缺金额不是 0；余额与软配额可透支，Key 已用额度不可负。
- 单值、最终存储或批量聚合越界整笔拒绝/回滚，不截断、不饱和。
  SQLite 精确聚合的中间精度也须与 PG 对齐，不能用 Rust 单值上限代替。

## 唯一结算权

```text
读取合格且账户归属正确的 pending 事实
  → 插入此前不存在的 UUID 回执
  → 仅按本事务新插入回执聚合用户余额扣减和 Key 已用额度增加
  → 校验账户更新集合与认领集合一致
  → 删除对应 pending
  → COMMIT
  → 通知进程内 soft-quota
```

插入回执不等于先提交；任何一步失败，回执、余额、额度、pending 全部回滚。
相同 UUID 顺序、并发或回复丢失重试只产生一次账户效果。
PG 按稳定 UUID 顺序锁账户；死锁/序列化失败重试整个事务，不单独重放扣款语句。
提交后的缓存通知丢失不改变数据库事实，由现有重载恢复。

pending 的 `(completed_at,request_id)` 索引避免重复扫描全部已结算历史；
仅首次新事实创建工作项，重复物化不能把已完成工作重新加入。
这是“至少一次投递 + 幂等账户效果”，不是上游调用恰好一次。

## 流水线与重放

```text
intent / slot → terminal spool
  → durable ingress（PG COPY / SQLite 批量 INSERT）
  → fact + attribution + pending + readiness 同事务
     ├─ 结算、财务统计、Codex 窗口费用
     └─ 日志投影 → 完成后 ack ingress
```

应用 worker 只处理事件批次、不透明 receipt 和语义结果，不理解表名、序号、COPY 编码或 SQL。
计量/投影独立退避；坏 journal 或财务冲突不能标成 ready，展示字段失败只延迟投影。
事实提交与 ready 标记必须原子，日志只读取 ready 条目，ack 必须已有事实和投影。
日志故障仍会积压 ingress 并消耗同一数据库资源：逻辑解耦不等于无限故障隔离。

| 中断位置 | 恢复规则 |
| --- | --- |
| slot 完整、尚未接收 | 用原 UUID 重放；缺可信终态只保留待核对 |
| 接收提交后、checkpoint 前 | 重放整批；接收确认必须代表整批提交 |
| 计量提交前 | fact/pending/ready 一起回滚，ingress 保留 |
| 计量提交后、投影前 | 结算继续，日志重试，不重算费用 |
| 投影写入后、ack 前 | UUID 幂等重放；不覆盖已接受日志 |
| 结算提交后、回复前 | 查询已有回执，不因超时再扣款 |

同 UUID 必须比较规范化身份、usage、价格、费用及归属，不只 ON CONFLICT DO NOTHING。
等价重放成功；不同财务内容保留并告警，不覆盖、不冲正、不二次扣款。
旧 journal 缺省值、失败归零和时间/金额列语义在比较前归一化。
非财务日志冲突另行保留，不撤销有效结算。

Console billed_at 从回执 LEFT JOIN 派生；费用统计/排行榜/凭证窗口来自事实。
保持时间、source、权限、NULL 和聚合精度口径，不把“有成本”改为“已结算”。
事实先于日志可见是明确的最终一致性行为。

没有日志 TTL 或事实清理任务。未来删除投影也不得删除财务事实；
在重放水位/幂等墓碑设计完成前，事实与唯一回执均不能清理。
只保留事实而删回执同样会重新开放扣款权。

## 迁移与回退

0063 把历史日志认领切换为事实/回执，不采用双表双认领，也不支持新旧结算者滚动混跑。

1. 隔离副本预检 UUID、已 billed、金额分组、缺 usage、零费用、墓碑、账户异常和积压；
   测量回填、锁等待、WAL/额外空间，确认真实停机预算。
2. 停止全部旧流量、在途和 worker，保留未知 intent；成对备份数据库与全部实例 spool。
   不丢弃未知证据来制造排空。
3. 在 migration lock 和既有原子批次内回填事实；已 billed 复制原回执金额/时间，
   不更新账户。未 billed 的合格行进入 pending，缺价格/费用按分类保存。
4. 逐 UUID 核对覆盖、财务字段、分类与分组金额，不只比较总和。非法旧已结算行使整批失败。
5. 删除日志物理 billed_at；旧认领 SQL 失败。保护 ingress，拒绝无事实/ready/投影的提前 ack。
6. 全部实例使用同版新二进制与原 spool，核对回执、账户增量及积压后恢复流量。

不修改已部署 migration、不新增 no-transaction 或 PG 提交屏障；保留历史 0034/0046。
迁移未提交可留在旧 schema；提交后优先 forward-fix，不能只回退二进制。
整体恢复必须停写，恢复同一时点数据库与所有 spool/WAL，并核对恢复点之后的上游使用。
若单事务回填无法满足容量预算，应停止发布并另行设计在线迁移，不临时拆事务或弱化认领。
小数据工具及其测量局限见[0063 成对备份恢复演练](persistence-rehearsal.md)。

## 验证与设计背景

### 按风险选择证据

| 风险 | 必须覆盖的契约 |
| --- | --- |
| 金额 | 中点上下、分项顺序、最大金额、批量溢出、现价变化后重放 |
| 扣款权 | 同 UUID 顺序/并发/回复丢失重试；在回执/用户/Key 更新后失败全部回滚 |
| 资格 | 成功缺 usage、策略归零、rejected、探测、invalid、账户不匹配、intent-only |
| 数据库保护 | 直接 SQL 伪造回执/修改事实被拒绝；墓碑身份仍可结算 |
| 阶段故障 | checkpoint/ready/ack 边界；日志锁住或展示非法仍可结算 |
| 冲突 | 财务与展示冲突分开保留，重启无双扣 |
| 升级 | 旧 journal、重复 ingress、已 billed 回填不扣款、在途旧认领等待、失败整批回滚 |
| 控制面 | 版本冲突、编译/审计失败、取消回滚、提交后启动恢复 |
| Codex | PG guard 与 SQLite intent/CAS 各自真实约束、取消及不确定结果 |
| 查询 | billed 过滤、权限、source/时间边界、排行榜与费用先于日志可见 |

领域端口契约不替代真实 PG COPY/锁/迁移和 SQLite 文件/WAL/单写故障测试。
默认与 SQLite workspace、Console spec、系统 E2E 是不同证据入口；
转发变更另外遵守获授权的真实上游 smoke，历史授权不自动延续。
契约测试本身不需要付费调用或性能负载。

### 为什么这样分离

旧实现把 ingress、查询宽表、统计与结算集中到一个仓储，并以
request_logs.billed_at 作为认领权：查询投影失败会阻塞扣款，删除展示数据可能破坏恢复依据。
控制面和 Codex 的 PG 事务、HTTP 中的 SQLSTATE 也跨越了应用边界。

原实施按 P1 行为契约、P2 操作接口、P3 独立事实、P4 故障/备份演练完成。
这些是设计演进，不是待实施清单；SQLite 随后的完整后端另有专页。
完成标准始终是金额与恢复不变量，而不是文件更小或 Mock 测试通过。

## 相关资料

- [行为契约基线](persistence-contracts.md)、[操作接口](persistence-interfaces.md)
- [独立计量事实](independent-metering.md)、[日志耐久化](request-log-durability.md)
- [日志准入决策](request-log-admission-review.md)、[控制面软删除](soft-deletion.md)
- [系统 E2E](system-e2e.md)、[工程对比背景](../reference/monoize-engineering-study.md)
