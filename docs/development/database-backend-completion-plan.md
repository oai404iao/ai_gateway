# 数据库后端抽象与 SQLite 完成总计划

> 状态：提案。本文是数据库后端抽象和生产级 SQLite 的控制性实施路线图，不描述当前已交付行为。
> 当前行为与 schema 始终以代码、测试、`migrations/` 和
> [数据库与控制面架构](database-architecture.md) 为准。

## 1. 权威边界与目标

本文冻结后续实施 PR 的范围、依赖、门禁和完成定义。它对“如何完成”具有路线图优先级，但不能覆盖
当前实现或 migration；若实现发现基线有变化，应先更新本文，再调整里程碑。

目标是让 SQLite 在首个受支持拓扑内具备 **PostgreSQL 当前全部功能的语义等价能力**，包括 Console
身份与控制面、运行时快照、路由、MCP、Codex OAuth、耐久请求日志、结算、统计和排行榜。SQLite
不是删减版、开发版或“只转发不计费”模式。PostgreSQL 继续是默认后端，也是多进程与现有生产部署
的默认选择。

本计划基线固定在 `ea365115f2b381295ae4c0e8ac1cdf05bf2460a1`。后续新增 PostgreSQL
功能或 schema 时，必须同时进入本计划尚未完成的 SQLite 契约，不能以“计划创建时不存在”为由
跳过。

## 2. 当前基线与激活边界

### 2.1 已有基础

在上述基线，`sqlite-backend` 已实现：

- 独立 SQLite schema/migrator、UUID/时间/JSON/数组/精确 Decimal 存储 adapter 和 schema
  对照测试；
- 一个事务内读取完整 `RuntimeConfigRecords` 并复用现有 compiler 的 runtime snapshot
  仓储；
- Console login/session/account/registration 仓储。按公开 async 仓储入口统计约有 24 个方法，
  覆盖认证、refresh rotation/replay、Profile、密码、bootstrap、邀请码和邀请流程；若把内部事务
  helper、读写变体或 facade 方法分别计数，数量会不同，因此该数字只用于规模盘点，不是稳定契约。

尚不可用的部分包括：从 `[database].url` 选择 SQLite、进程运行时 dispatch、完整控制面读写、
Codex 仓储与投影、请求日志 ingest/final projection、结算、查询、统计和排行榜。默认构建不包含
SQLite，`sqlite:` URL 仍被拒绝。

### 2.2 冻结的首个生产拓扑

首个生产 SQLite 目标只支持：

```text
一个 Gateway 进程
  -> 一个本机持久化 SQLite 数据库文件
  -> 同一主机上的本地持久化 spool / Images 临时文件
```

- live 数据库、WAL、spool 和 Images 临时文件必须位于经正向允许的本地文件系统。备份不受“必须
  留在本机”的限制：一致备份应先在受控边界内生成，再加密并复制到受访问控制的异机或对象存储。
- 不支持 NFS、SMB/CIFS、分布式文件系统、共享数据库文件、多个 Gateway 副本或 HA。
- 不承诺多进程读写、自动故障转移、在线跨主机复制或 PostgreSQL 到 SQLite 的在线迁移。
- 进程排他锁是受支持拓扑的组成部分，不是多副本协调机制。其保证只适用于 M2 冻结并验证的生产
  平台、内核锁语义和本地文件系统；不能把规范路径或路径旁路锁的行为外推为所有平台/文件系统上的
  通用保证。

### 2.3 激活门禁

在里程碑 M10 全栈收敛完成前，正常配置解析必须继续拒绝 `sqlite:` URL；测试只能通过 crate-private
构造器或明确的测试入口连接 SQLite。M10 通过也不等于可部署。I-12 是一个 PR，但必须有两个清晰
阶段：先在激活代码不变的情况下通过并记录全部预激活门禁；评审确认后才提交该 PR 内最后一个最小
激活改动，开放文件型 SQLite URL、接入单独的 SQLite 部署示例，并原样重跑最终门禁。最终官方
Docker/Release 二进制必须编译 `sqlite-backend`；Cargo 默认 feature 可以继续只提供 PostgreSQL。
独立 SQLite 部署示例可作为该未合并 PR 的 gate 输入随激活改动验证；README、用户文档和发布宣传
中的可部署声明只能在激活后 gate 通过后才完成并随 PR 发布，且最终 activation 的运行时代码改动
仍须保持最小。

## 3. 目标架构

### 3.1 闭合枚举 facade

保留以下应用可见名称和职责：

- `DatabaseConnectOptions`
- `DatabasePool`
- `RepositoryTransaction`
- `AuthRepository`
- `ControlPlaneRepository`
- `RequestLogRepository`

每个类型内部使用私有、闭合的 `Postgres | Sqlite` 枚举 dispatch。具体连接选项、pool、transaction、
row mapping、SQL 和批量写入只存在于 `src/persistence/postgres/` 或
`src/persistence/sqlite/`。`src/persistence.rs` 继续作为后端中立重导出面。

明确拒绝以下方案：

- `sqlx::Any` / `AnyPool`：它会隐藏 SQL 方言、事务和类型差异；
- 把 backend type parameter 扩散到 application、HTTP、MCP 或 worker 的 pervasive generics；
- `dyn` async repository trait 和由此产生的对象安全、生命周期、分配与错误擦除复杂度；
- 在 application、HTTP、MCP、runtime compiler 或 worker 中按后端 `match`。

后端分支只允许位于 persistence facade 和数据库生命周期层。测试 harness 可以使用泛型或宏复用
同一契约，但不能使生产调用方泛型化。

### 3.2 数据库生命周期

验证后的 `DatabaseConnectOptions` 只能消费一次来创建唯一生命周期 owner
`DatabaseRuntime`（若实现最终采用 `DatabasePools`，职责不变）。该 owner 同时持有路径旁路创建
锁、绑定稳定数据库文件身份/描述符的 OS 排他锁、共享 writer coordinator、专用 lifecycle
connection、控制面与请求日志两个逻辑 pool，并独占 migration、checkpoint 和有序 shutdown。
`main`、serve/bootstrap/reset 等 CLI 和 worker 只能接收该 owner 或其受控 handle，不能各自再次
`connect`、各拿一把锁或独立管理 pool。

该 owner 及其连接工厂必须同时满足：

1. **URL 与 feature**
   - PostgreSQL URL 保持现有 `postgres://` / `postgresql://` 行为。
   - SQLite 生产入口只接受 `sqlite:///absolute/path/to/database.db` 形式、指向普通持久文件的
     URL；首发不接受任何 query parameter。拒绝相对路径、非空 authority（含 host、userinfo
     或 port）、query、fragment、`:memory:`、临时数据库和空路径。
   - URL path 按严格 URI 规则只 percent-decode 一次；拒绝错误转义、非 UTF-8、NUL 和解码后
     的 `.`/`..` 路径段。解码结果必须已是规范化绝对路径。对已存在的父路径逐段 no-follow
     解析并取得规范路径，数据库 leaf 可以尚不存在；连接、旁路锁和进程内身份键都使用同一个
     规范化结果。规范路径和路径旁路锁只负责安全创建与常见路径别名收敛，不能单独防住 hard-link
     或 bind/mount alias；单实例约束最终必须绑定下述稳定文件身份和数据库文件描述符上的 OS 锁。
   - 测试可使用独立的 in-memory 构造器，但该构造器不从 TOML 暴露。
   - 未编译 `sqlite-backend` 时，SQLite URL 返回明确的 feature-disabled 配置错误。
   - `[database].password_file` 仅适用于 PostgreSQL；SQLite 同时设置它必须报错，而不是忽略。
2. **持久性 pragma**
   - `synchronous` 是 connection-scoped，不能只在文件初始化时设置。lifecycle connection、
     控制面 pool 和请求日志 pool 的**每个物理连接**都必须在首次可用前设置并读回验证
     `synchronous=FULL`、`foreign_keys=ON` 和统一 busy policy（含固定的 busy timeout）；pool
     after-connect/reconnect 以及 checkout/recycle 后再次交付前的 gate 必须重复同一设置与验证，
     任何不匹配都销毁该连接并以 typed storage/configuration failure fail closed，不能把它交给
     调用方；
   - 文件初始化设置并验证 `journal_mode=WAL`；每个新建、重开或回收后的 lifecycle/control/log
     连接还必须验证其观察到 WAL，并完成上述 connection-scoped pragma/busy policy gate；
   - 不允许调用方通过 URL 参数降低这些值。
3. **文件安全**
   - 创建前按受支持生产平台的文件系统类型做正向本地 allowlist 分类。已知远程/不支持类型以及
     无法识别的类型都 fail closed；不宣称能在任意平台普遍识别所有远程文件系统。M2 退出前必须
     冻结平台、文件系统、稳定文件身份和 OS 锁语义的正向 allowlist，并验证官方容器采用的本地卷
     策略；无法可靠取得/复验平台等价的稳定身份或建立 identity-following 排他锁时 fail closed，
     该组合不得列为受支持生产环境；
   - Unix 下父目录和数据库/lock/WAL 相关文件由 Gateway 运行用户拥有，目标权限分别为
     `0700` 和 `0600`，拒绝 symlink 与不安全的 group/world 可写路径。数据库、WAL、SHM 和路径
     旁路 lock 文件一旦存在，在平台提供可靠 link-count 语义时还必须拒绝 hard-link count
     `!= 1`；冻结的生产平台/文件系统必须具备并通过该能力，否则不受支持；
   - 数据库 leaf 必须通过 no-follow、原子且仅创建普通文件的安全打开/创建流程取得描述符，不能先
     做 path check 再由 SQLx 按路径重新打开而留下替换窗口。`DatabaseRuntime` 记录该描述符对应
     的稳定文件身份（Unix 上为 device + inode，其他冻结平台使用经验证的等价标识），并在路径
     旁路创建锁之外，对绑定数据库文件描述符/身份的 OS 锁持有排他锁。alternate path、
     hard-link 和 bind/mount alias 只要在冻结的受支持平台/文件系统上到达同一身份，就必须进入
     同一 identity exclusion domain，并因 identity lock 竞争或更早的 link-count/path 安全校验
     快速失败，绝不能同时启动；
   - 创建、已有文件与 link-count 校验、身份取得/复验、alias 锁竞争、容器 UID/GID 和备份恢复后
     的权限具有确定测试。本文不宣称未冻结平台、内核或文件系统上的 alias 锁行为。
4. **逻辑 pool**
   - 控制面 pool 与请求日志 pool 仍是两个逻辑容量和指标域，但连接同一个 SQLite 文件；
   - 两者共享一个进程内公平 writer coordinator，避免日志批次长期饿死控制面或反之；
   - pool 指标保留 size/idle/acquire pressure，并增加 backend、busy/lock wait、checkpoint
     结果和 writer queue 可观察性；这些 pool/writer 指标由 M2 提供，不得把两个 pool 的容量
     误报为可并行 writer 数。
5. **migration、锁与关闭**
   - `DatabaseRuntime` 只执行一次 `run_migrations`，并按枚举 dispatch 到各自 migrator；
   - 启动时先对 `0700` 父目录和数据库路径做逐段 no-follow 安全预检，以路径旁路创建锁串行化
     尚不存在 leaf 的创建，再按上述 no-follow 流程安全创建/打开数据库、记录稳定身份并取得绑定
     文件描述符/身份的 OS 排他锁。持有 identity lock 后必须重新 no-follow 解析数据库路径，
     复验 path leaf 与已锁描述符仍是同一稳定身份，并复验 owner/mode/link count。初次复验通过后
     才能打开专用 lifecycle connection 并创建/验证 WAL；WAL/SHM 创建后、control/log pool 启动
     和 migration 前，必须再次完成适用文件校验并复验锁定数据库的 path/identity。任一身份、
     路径或能力无法确认都 fail closed。`0700` 且受信 owner 的父目录和全程 no-follow 约束共同
     防止复验后的 leaf replacement；
   - 只有上述第二次复验通过后才可启动 control/log pool、执行 migration 和启动 worker。在冻结
     支持矩阵上，同路径、编码路径别名、alternate path、hard-link 或 bind/mount alias 到达同一
     身份的第二个进程都必须因 identity lock 竞争或前置安全校验快速失败；路径旁路锁不能作为这一
     保证的替代；
   - 关闭时先停止新请求，再按日志流水线顺序 drain 并阻止 pool 新 checkout；保留专用 lifecycle
     connection 执行有界 WAL checkpoint 和必要同步，之后关闭最后连接，最后释放进程锁；
     identity lock 及其安全文件描述符必须至少保持到所有 pool/连接关闭且 checkpoint 完成之后，
     才能与路径旁路锁按定义顺序释放；
   - checkpoint 失败要返回/记录 typed failure，但不得删除仍可恢复的 WAL、spool 或 ingress。
6. **错误**
   - `RepositoryError` 扩展为后端中立的 not-found、conflict、constraint、busy/timeout、
     backend-mismatch、corrupt/storage-unavailable 和 migration 类别；
   - SQLx 后端错误只在 adapter 层分类，HTTP/application 不解析数据库错误字符串。
7. **备份与恢复**
   - 禁止把正在写入的 `.db`/WAL 文件直接复制后称为一致备份。先建立有界 backup barrier、同步
     spool 并固定 manifest，再使用 SQLite backup API 捕获数据库，或在该 barrier 内执行协调
     checkpoint 后捕获；
   - 备份 manifest 记录数据库 generation、spool 文件集和 checkpoint，使 restore 后允许依靠
     UUID 幂等语义重放但不遗漏未投影日志；
   - 备份在离开运行主机前加密并限制读取/写入权限，随后复制到受保护的异机或对象存储；恢复必须
     同时校验数据库与 spool manifest、文件 owner/mode 和密钥可用性。

### 3.3 事务意图与不变量

facade 必须表达事务**意图**，而不是让上层选择 SQL：

- **一致读快照**：runtime records 和需要跨多表一致性的查询在一个 read transaction 中完成；
- **管理写入**：PostgreSQL 使用 `SERIALIZABLE` 事务，SQLite 使用受 writer coordinator 约束的
  `BEGIN IMMEDIATE`；serialization/busy 冲突只在尚未发布状态时做有界重试；
- **日志写入/结算**：使用专用短事务和幂等 claim，不借用控制面长事务。

必须保持：

1. transaction 只能传给相同 backend/pool 创建的 repository；实例不匹配返回
   `backend-mismatch`，不得 panic 或错误访问另一枚举分支。
2. SQLite writer transaction 必须短。持锁期间禁止 provider HTTP、DNS、文件大 I/O、密码哈希、
   无界聚合或其他无上限 CPU-heavy 工作。唯一有意保留的有界例外是管理写入后从同一 transaction
   读取**完整候选配置并编译** `CompiledRuntimeConfig`；它不得被改成事务外读取，也不引入 CAS
   重设计。M5 必须冻结可支持配置的记录数/序列化字节等数值上限、writer queue wait、writer
   lock-held duration 与 transaction duration telemetry，以及各自的数值 pass/fail 阈值；超限
   候选在 commit 前失败。M10 把这些限制和阈值纳入全栈资格 profile 并验证。
3. 控制面发布固定顺序为：在事务内写候选记录 → 从同一事务读取完整候选记录并编译
   `CompiledRuntimeConfig` → 写脱敏 audit → commit → 通过 `ArcSwap` publish。编译或 audit
   失败必须 rollback；commit 前不得 publish。
4. commit 后 publish 使用已经验证的候选快照；跨进程 PostgreSQL 收敛仍由 reload worker 负责，
   SQLite 首期只有一个进程，但仍复用相同路径。

## 4. 专项语义要求

### 4.1 Codex 事务重构前置条件

当前 Codex refresh/reset 路径可能在 PostgreSQL row lock transaction 中调用 provider。SQLite
writer lock 不能跨外部 HTTP，因此 **先重构 PostgreSQL，再实现 SQLite**：

- refresh 使用 durable claim/lease、`refresh_generation`、dispatched 状态和 CAS：
  1. 短事务按 expected generation 取得带 token 的 pre-dispatch claim；
  2. 在 provider 请求可能发出前，先用短事务把该 generation 持久标记为 `dispatched`，再在事务外
     调用 provider；
  3. 新短事务按 claim token + generation CAS 写 rotating token、identity 和最终状态；
  4. 只有可证明从未进入 `dispatched` 的 claim 才可安全释放或重取。`dispatched` 后进程崩溃、
     lease 到期或工作被放弃时，provider 可能已经轮换 token，而新 token 尚未持久化；该窗口无法
     由 generation/CAS 自动恢复，必须持久转入 `refresh_outcome_unknown` / `reauth_required`
     并停止自动 refresh。除非权威 provider 契约明确保证 refresh 的幂等或旧 token 可复用，且
     该契约已被版本固定和测试，否则绝不自动重试旧 refresh token。即使实际请求尚未离开进程，
     “已标记 dispatched 但无法证明未发送”也按未知结果 fail safe；
  5. 迟到结果不能覆盖新 generation，也不能把 unknown/reauth 状态静默改回可用。
     rotating refresh 的 unknown 恢复边界始终是重新授权；下述 reset outcome
     reconciliation 只处理 reset-credit operation，绝不能用来“确认”或恢复未知的 rotating
     refresh token。
- provider quota HTTP shape 保持，HTTP 仍在事务外；但 `checked_at` 是客户端请求开始时间，不能
  代表 provider 处理顺序。reset 不再持有 row lock 后，quota **持久化顺序**必须改为 durable
  request-attempt/observation/fence 协议，不能把旧 apply 逻辑原样搬到锁外：
  1. 每次 quota HTTP attempt 必须在 dispatch 前用短事务注册，记录 credential、单调 attempt
     version、attempt ID、`registered`/`dispatched`/`completed`/`expired` state、当前 fence
     generation 和 claim/lease；注册或 `dispatched` 事实未提交就不得发送。响应只能按
     attempt ID + version + fence generation，从 `dispatched` state CAS 完成该 attempt 并保存完整
     规范化 observation；`registered` 只能被取消/expire，伪造或错配的 pre-dispatch completion
     必须拒绝。已 `completed` 的同 payload 重放幂等读取，payload 冲突或 generation 不匹配 fail
     closed。`expired` 是 immutable terminal attempt state，late response 即使在 drain/close 前到达
     也必须拒绝，不能改写 classification 或 cursor。`checked_at` 可保留为 payload 元数据，但 fence
     内绝不用于推导 provider 先后；
  2. reset operation 创建必须与 quota attempt 注册/apply 在同一 credential 上串行化，创建新的
     fence generation，并原子快照最后已 apply version 以及所有已注册或已 dispatched、尚未完成的
     attempt。reset 每次 provider dispatch 和 terminal transition 都写 durable boundary。任何
     quota attempt 只要跨越 reset dispatch boundary 或 terminal boundary（包括“quota 请求先开始，
     provider 在 reset 后才处理”），其 observation 一律标为 `ambiguous`，只能保留作诊断，永远
     不能用来判定该次 rollover 是 `manual` 或 `openai_official`；
  3. reset 达到 terminal 时，terminal operation、确认成功时的 reset event、audit 和 fence
     terminal marker 必须在一个短事务原子提交；失败 terminal 不创建 reset event。terminal
     marker 提交后，fence 必须持有独立、单调递增的 `anchor_generation`。每个 generation 只能有
     一个 active anchor claim/lease，且该 claim 注册一个新的 post-terminal quota attempt；不能把
     terminal 前已注册的迟到响应提升为 anchor。只有当前最新 generation 的 durable successful
     observation 才能成为 fresh ordering anchor。anchor worker 必须先持久化 claim，再持久化
     dispatched，最后在事务外请求 provider；
  4. 当前 fence generation 内所有 terminal boundary 前注册的 in-flight attempt 均已完成且持久
     归类，或在有界 lease 到期后持久标为 `expired` 且归类为 `ambiguous`/`stale`。anchor 请求
     timeout、worker
     crash、lease expiry 或 durable failure 只能把**该 anchor generation** CAS 为
     `expired`/`failed`，不得关闭或放开 fence；recovery worker 竞争时只能有一个 worker CAS 创建
     下一 `anchor_generation` 及其 active claim/lease，并发送新的 post-terminal quota 请求。旧
     generation 后到的 provider response 必须按 fence generation + anchor generation + attempt
     ID 拒绝，即使新 generation 尚未成功也不能提升为 anchor、写 history 或移动 cursor；
  5. 最新 anchor generation 的 successful observation 以及 cutoff 内全部严格有序 post-terminal
     observations 均已持久化前，不得开始向 quota/window history drain。anchor 成功后，其后的
     quota attempt 必须逐个注册、dispatch、完成，或由等价的 durable single-flight coordinator
     保证 provider 观察严格有序。准备 drain 时必须先以原子 `active -> draining` CAS seal 当前
     fence generation，并在同一事务捕获最大已注册 attempt version 作为 drain cutoff；seal 后新
     quota 请求必须等待/重试，不能挂入该 generation 或绕到 fence 外 apply。ready 后 drain 只按
     durable attempt version/state、fence generation 和胜出的 anchor generation 推进，不按
     `checked_at` 排序。明确在首个 reset dispatch marker 前完成的 observation 可按 reset 前事实
     处理；所有跨 dispatch/terminal boundary 的 observation 不参与 rollover 分类；成功 reset
     event 只可与胜出的 fresh anchor 及其后严格有序的 observation 匹配并归类为 `manual`，失败
     terminal 则在没有 reset event 的情况下从 fresh anchor 恢复正常分类。attempt state、
     classification 和 drain cursor 每步同事务提交；
  6. 上述 readiness 成立且全部 candidate drain 完成后才可原子关闭 fence。关闭时保留 fence
     generation 与 attempt terminal facts；之后到达的过期/旧 generation 响应必须按 attempt ID +
     fence generation + anchor generation 拒绝，不能重开 fence、写 quota/window history 或移动
     cursor。provider 持续不可用时，fence 必须保持 visible、active 和 alerted，暴露最新
     `anchor_generation`、失败原因与重试状态；不能因 timeout/失败次数静默 open。provider 恢复后
     recovery worker 必须能通过下一 generation 继续推进，最终完成 drain/close；
  7. 只有不存在 active/draining fence 时才继续使用既有 stale `checked_at`/version CAS guard。
     pending/dispatched/recovering reset 必须可见、可告警并使用同一 provider 幂等请求恢复；
     unknown reset 必须保持可见和 fenced，不能重新 dispatch reset，只能通过下述显式 operator
     reconciliation 进入 terminal resolution。任何状态都不能靠超时猜测失败后关闭 fence 或放开
     分类。
- 手工 reset-credit 的 Console API 新增明确的耐久幂等边界。`POST /providers/codex-oauth/credentials/{id}/quota/reset`
  必须要求 `Idempotency-Key` Header；Console UI 在用户确认一次新意图时用
  `crypto.randomUUID()` 生成一次，并在该 pending intent 的网络、`202` 和服务重启重试中复用；
  client 不得在每次 HTTP attempt 内生成 key。UI 将非 secret 的 pending intent/key 保留到
  terminal；关闭对话框不能取消 durable operation，重开时必须恢复它。terminal 后用户发起新的
  确认意图才生成新 key。
- 创建 operation 的短事务持久化全局唯一 `Idempotency-Key`，并把它与稳定 Console user UUID
  actor（不是 JWT/Session ID）、credential 和 action intent fingerprint 绑定，同时生成且永久
  绑定唯一 provider `redeem_request_id`。`(actor, credential, Idempotency-Key)` 永远只映射一个
  `redeem_request_id`。同 key 且 actor/credential/action 全部相同才返回或恢复同一
  pending/terminal operation；同 key 换 actor、credential 或 action 返回 conflict，绝不再调用
  provider。同一 credential 已有 active pending/dispatched/recovering 或 unknown unresolved
  operation 时，不同 key 也先 conflict，不能并行创建第二次消费；审计引用同一 operation。
- provider 调用只发生在 durable attempt claim 提交后并位于事务外。operation 始终复用一个稳定
  `redeem_request_id`，但每次首次调用或恢复重试都必须取得新的单调 attempt generation、唯一
  claim token 和有界 lease；数据库约束保证每个 operation 同时只有一个 active attempt。
  dispatch 前先按 claim token + generation 把 attempt 持久标记为 `dispatched`。lease 到期只能
  让 recovery worker CAS 接管为下一 generation，不能让两个 provider attempt 同时成为 active；
  所有 generation 都保留 durable 状态。outcome unknown 不能直接接受普通 mutation 或 provider
  completion，也不能再以同一 `redeem_request_id` dispatch；只能按已核实 provider 事实通过下述
  审计 CAS 进入明确 resolution/terminal path。
- provider 响应的 terminal finalization 必须从“operation 仍处于 non-terminal pending family
  （pending/dispatched/recovering），且该 generation 仍是 active attempt 并已处于 durable
  `dispatched` state”做 compare-and-set；pre-dispatch claim 只能取消/过期，不能接受
  provider-derived terminal completion。`dispatched` 同时是 attempt state 和 operation 的
  pending-family 可见投影，不能成为 CAS 死角。成功 CAS 的同一个短事务原子提交 immutable
  terminal operation/result、适用时唯一的 reset event、脱敏 audit 和 quota fence terminal
  marker；这些事实分别以 operation ID 唯一，重复执行不能产生第二行。失败 terminal 不创建 reset
  event。
  一旦 terminal，任何路径都不能改写结果。旧 generation 在新 attempt 已胜出后迟到时必须读取并
  返回该 immutable terminal result，不能覆盖 operation、重复 event/audit 或移动 fence marker；
  若新 attempt 尚未 terminal，则旧 generation 只能返回当前 pending 状态并停止 finalization。
  provider 对同一 `redeem_request_id` 返回 `already_redeemed` 时，必须按冻结的 provider
  幂等契约与原始成功响应收敛到同一 terminal success，而不能创建第二次 reset 或把已成功操作改成
  failure。
- 超时、崩溃或 HTTP 响应丢失后必须复用已持久化 `redeem_request_id` 并通过上述 claim generation
  恢复；不能为同一 Console key 生成第二个 provider 请求 ID。terminal DB commit 后重复同 key
  直接返回已保存结果。同步完成时尽量保持现有
  `CodexQuotaResetResponse` 的 `200` shape；OpenAPI 另外明确 required Header、pending/retry 的
  `202`（含稳定 operation 状态和 `Retry-After`）及 intent/key reuse 的 `409`；这是明确的
  Console contract 可见变更。
- I-08 必须提供一个**先写 OpenAPI、仅 admin 可用且经过 Console JWT 认证**的 reset outcome
  reconciliation 接口；直接编辑数据库不是受支持的恢复方式。相对 `/console/v1` 的固定资源为：
  - `GET /providers/codex-oauth/quota-reset-operations`：按 `credential_id`、operation status
    分页/排序列出操作，使管理员可从 credential 查到 unresolved operation；
  - `GET /providers/codex-oauth/quota-reset-operations/{operation_id}`：返回 operation、reset
    attempt、quota fence、最新 `anchor_generation` 和 resolution 状态的脱敏视图及强 ETag；
  - `POST /providers/codex-oauth/quota-reset-operations/{operation_id}/resolution`：要求
    `If-Match` 和 `Idempotency-Key`，body 的 `outcome` 只允许 `confirmed_reset` 或
    `confirmed_not_reset`，并包含必填 `reason`（1–500 字符）及可选结构化
    `evidence`（`provider_reference` 最长 128 字符、`observed_at`、`note` 最长 1000 字符）。
    不接受任意 provider response/Header、access/refresh token、Authorization、cookie 或其他 raw
    provider secret；reason/evidence 先经控制字符、secret-shaped input validation 和统一
    redaction，命中 secret 模式即拒绝，审计只保存通过校验的有界结构。
- resolution 只适用于 reset-credit operation 的 durable `outcome_unknown`，不是 rotating
  refresh unknown 的恢复入口。仓储先按全局唯一 resolution `Idempotency-Key` 查重，再把 key
  绑定到 admin actor UUID、operation ID、outcome 和规范化 reason/evidence fingerprint；相同 key
  与相同请求即使前次 HTTP response 丢失也返回完全相同的 immutable resolution result。相同 key
  换 actor/operation/payload、不同 key 试图改写已 resolution 的 operation、错误 `If-Match` 或
  并发 CAS 败者均返回明确 conflict/stale response，绝不能生成第二份 event/audit/anchor side
  effect。
- 胜出的 versioned CAS 在一个短事务内提交 immutable resolution、terminal operation/result、
  脱敏且不可变的 audit 和 fence terminal boundary。`confirmed_reset` 还按 operation ID 唯一约束
  **只提交一次** manual reset event；`confirmed_not_reset` 不提交 reset event。两种 outcome 都
  不关闭 fence，而是在同一事务把 fence 留为 active，并创建或重启下一个单调
  `anchor_generation` 及其唯一 active claim/lease；provider quota HTTP 随后由 worker 在事务外
  dispatch。后续只按最新 generation 成功 observation 执行上述 ordered drain/closure。
- OpenAPI 必须固定 admin authorization、filter/pagination、ETag/`If-Match`、required
  `Idempotency-Key`、request bounds、枚举、成功 replay 和 conflict/stale/error responses；同步
  生成 TypeScript types，更新 Console client，并提供管理员 unresolved-operation list/detail 与
  resolution UI。component/spec/E2E 必须覆盖权限、两种 outcome、validation、ETag conflict、
  HTTP response-loss 同 key retry、两个管理员并发 resolution、不可变 audit redaction，以及
  resolution 后 anchor replacement/最终 fence closure。I-08 同时交付 operator runbook：只能先
  从 provider 的非 secret 权威事实核验，再经此接口解决；禁止 SQL 手改，并明确 rotating refresh
  unknown 仍须 reauthorization。
- reset operation 为 pending、dispatched、recovering 或 outcome unknown 且未显式解决时，或其
  terminal quota fence 仍为 active/draining 时，credential 上存在 destructive-mutation fence。
  credential delete/tombstone、token 或 identity 替换/reimport、connector-pool reassignment，
  以及包含任一此类变更的 batch 必须在写入任何成员前以 typed conflict 整批拒绝；不能通过删除、
  重导入或移动 pool 丢弃 operation/fence。unknown outcome 必须由 operator 通过显式、耐久、可审计
  的 reconciliation/resolution 状态机确认；operator 若直接核实 terminal outcome，也必须遵守上述
  operation/event/audit/fence marker 原子提交、operation ID 唯一和 immutable result 规则。即使
  operation 已解决，destructive fence 也要等其 quota fence 完整 drain/close 后才能解除，超时或
  普通 credential mutation 不得静默解除。
  credential 日后合法 tombstone 时，reset operation、attempt、event 和 audit 历史仍按 operation
  ID 保留，外键/清理不得级联删除。
- PostgreSQL 新路径先通过并发、崩溃和等价性测试，旧“transaction 跨 HTTP”路径删除后，SQLite
  才可开始。
- SQLite 不模拟 PostgreSQL projection trigger；Responses/Images managed channel、credential
  和 `codex_oauth_credential_channels` 必须在同一个 `BEGIN IMMEDIATE` 中显式创建、更新、
  tombstone，并维持各格式权限和健康隔离。

### 4.2 请求日志、计费与崩溃恢复

SQLite 必须保留现有边界，不得把三段流水线压成直接写 `request_logs`：

```text
local spool -> request_log_ingest -> request_logs -> settlement
```

- SQLite ingestion 用幂等批量 `INSERT` 替代 PostgreSQL `COPY`，但 **ingress commit 前不得推进
  spool checkpoint**。
- final projection 在一个事务内幂等写 `request_logs`；**final commit 前不得删除 ingress**。
  可在同一事务删除，或提交 final 后以第二个幂等事务删除，但不能反序。
- settlement 以未结算事实取得唯一 claim，在同一短事务写用户余额、API Key 已用额度和
  `billed_at`；进程内 soft quota 只在 commit 后更新。
- 所有金额、价格、倍率和聚合使用 Rust `Decimal` 精确 fold，并按规范化 scale 写 TEXT。
  禁止 SQLite `REAL`、Rust `f64`、`SUM(TEXT)`、隐式 numeric coercion 或先转浮点再格式化。
- batch 必须按事务时长和 writer 公平性有界；高 backlog 不能无限占有 SQLite writer lock。

至少覆盖以下 crash matrix：

| 故障点 | 恢复要求 |
| --- | --- |
| spool append/sync 前后 | 保持现有 group-sync 损失窗口说明；CRC 尾部可截断恢复 |
| ingress commit 前 | checkpoint 不前进，整批安全重放 |
| ingress commit 后、checkpoint 前 | UUID 幂等重放，不产生双 final |
| final insert 后、ingress delete 前 | final/ingress 状态可重试，不丢记录 |
| settlement claim 或余额写入中 | transaction 全回滚；重启后只结算一次 |
| settlement commit 后、soft quota 更新前 | DB 为恢复源，重载后收敛 |
| drain/checkpoint 中被终止 | spool、ingress、WAL 任一持久层仍可继续 |

### 4.3 查询、统计和排行榜

SQLite 必须覆盖 request-log 查询、个人 usage、渠道组状态、个人/系统花费、Codex 周期花费和
`Asia/Shanghai` 日/周/月排行榜：

- 时间桶、时区边界、连续空桶、P90/P50、成功率、过滤和排序与 PostgreSQL 响应契约一致；
- SQLite 不依赖 `generate_series`、PostgreSQL percentile、advisory lock 或 JSONB 运算；
  adapter 以索引限定/分页读取事实，在 Rust 中完成时间桶、percentile 和精确 Decimal fold；
- 查询必须有窗口/行数/内存上限和稳定排序，不能为了后端中立退化为无界全表加载；
- PostgreSQL 排行榜继续使用 advisory lock；SQLite 在单进程排他锁拓扑内用短
  `BEGIN IMMEDIATE` + durable refresh generation 协调，失败保留上一版。该设计不构成
  多进程支持；
- 两个后端对同一 canonical fixture 生成逐字段等价的 DTO 和排行榜快照。

本节不拥有 `/system/load` 或 request-log backlog/health：M2 只产出 pool/writer 指标，M6
拥有 backlog/health 并把两类指标接入 `SystemMetrics`；M7 只实现日志查询、usage、统计和排行榜。

## 5. 交付拓扑、依赖与 PR 数量

本计划本身是 planning PR（P-00），之后固定为 **11 个实施里程碑、12 个实施 PR**。M9 因凭证
生命周期与 quota/reset 风险不同拆成两个 PR。任何拆分、合并、换序或门禁削弱都必须先更新本文并
完成评审。

```text
P-00 本计划
  -> M1 / I-01 中立契约
    -> M2 / I-02 生命周期与 facade 基础
      +-> 控制面轨：M3 / I-03 -> M4 / I-04 -> M5 / I-05
      +-> 日志轨：  M6 / I-06 -> M7 / I-07
      +-> Codex 轨： M8 / I-08

M5 + M8       -> M9 / I-09
I-09 + M7     -> M9 / I-10
M5 + M7 + M9  -> M10 / I-11 -> M11 / I-12
```

M2 完成后，M3–M5、M6–M7 与 M8 可并行推进；随后 Codex 轨仍受跨轨依赖约束：I-09 必须等待 M5
和 M8 全部完成，I-10 必须等待 I-09 和 M7 全部完成。M10 只有在 M5、M7 和整个 M9 全部通过后
才能收敛。共享 facade、migration 与 qualification fixture 也需要串行协调，因此三轨只能部分
并行。不得从某条“最先完成”的轨提前开放 SQLite URL。

| 里程碑 | 实施 PR | 依赖 | 估算（工程日） | 风险 | 状态 |
| --- | --- | --- | ---: | --- | --- |
| M1 后端中立契约 | I-01 | P-00 | 5–8 | 中 | [x] 完成（[PR #135](https://github.com/oai404iao/ai_gateway/pull/135)，2026-08-18；实际 1 工程日） |
| M2 DB 生命周期与枚举 facade | I-02 | M1 | 9–13 | 高 | [ ] 未开始 |
| M3 控制面生命周期与读取 | I-03 | M2 | 7–10 | 中 | [ ] 未开始 |
| M4 身份与访问写入 | I-04 | M3 | 6–9 | 中 | [ ] 未开始 |
| M5 路由、系统与 MCP 写入 | I-05 | M4 | 9–13 | 高 | [ ] 未开始 |
| M6 耐久日志与结算 | I-06 | M2 | 12–17 | 高 | [ ] 未开始 |
| M7 查询、统计与排行榜 | I-07 | M6 | 10–15 | 高 | [ ] 未开始 |
| M8 Codex 跨 HTTP 事务重构 | I-08 | M2 | 15–22 | 高 | [ ] 未开始 |
| M9 SQLite Codex 完整实现 | I-09、I-10 | I-09: M5、M8；I-10: I-09、M7 | 15–23 | 高 | [ ] 未开始 |
| M10 全栈 facade/runtime 收敛 | I-11 | M5、M7、M9 | 8–12 | 高 | [ ] 未开始 |
| M11 生产启用与运维 | I-12 | M10 | 6–8 | 高 | [ ] 未开始 |

合计底部估算为约 102–150 工程日；按集成未知量对外表述为约 **100–150 工程日**。

## 6. 里程碑明细

### M1 / I-01：后端中立契约

- **范围**：盘点三个现有 repository 的公开方法；把 DTO、事务意图、错误类别、batch/result
  语义移到后端中立模块；固定闭合枚举 facade 设计和 backend-match 契约。不得连接 SQLite
  runtime。
- **依赖**：P-00。
- **关键文件/模块**：`src/persistence.rs`、`src/persistence/auth.rs`、
  `src/persistence/records.rs`、`src/persistence/error.rs`、三个 PostgreSQL repository。
- **进入条件**：基线 inventory 与当前 PostgreSQL tests 通过；每个公开调用方已有清单。
- **退出条件**：PostgreSQL 通过新契约且行为不变；没有 SQLx concrete type 泄漏到
  application/HTTP/MCP/workers；错误映射有单元测试。
- **必测**：默认 feature 的 unit/integration、compile-fail 或可见性测试、错误分类与 Decimal
  normalization；Rust 1.92 MSRV check。
- **风险/工作量**：中，5–8 日。主要风险是把 PostgreSQL 偶然语义误当公共契约。

### M2 / I-02：数据库生命周期与枚举 facade

- **范围**：实现 enum-backed `DatabaseConnectOptions`、`DatabasePool`、
  `RepositoryTransaction` 和三个 repository facade 骨架；由验证后的 options 一次创建唯一
  `DatabaseRuntime` 生命周期 owner，统一持有路径旁路创建锁、no-follow 安全打开的数据库描述符、
  稳定文件身份及绑定该描述符/身份的 OS 排他锁、共享 writer coordinator、lifecycle connection、
  两个逻辑 pool、migration 和 shutdown/checkpoint；规范路径或旁路锁本身不得被视为
  hard-link/bind/mount alias 排除机制。加入 SQLite pool 创建、覆盖
  首次建立/重开物理连接及 pool checkout/recycle 再交付的 per-connection pragma/busy policy
  setup + read-back gate、
  pool/writer metrics、权限、link-count、路径与锁定身份验证。配置入口仍拒绝 SQLite。
- **依赖**：M1。
- **关键文件/模块**：`src/persistence/database.rs`、`src/persistence.rs`、
  `src/persistence/postgres/`、`src/persistence/sqlite.rs`、startup/shutdown 和 metrics 的
  数据库接缝。
- **进入条件**：中立方法集与错误集冻结；生命周期 fault points 可注入。
- **退出条件**：crate-private SQLite runtime 可安全创建、迁移和关闭；serve/bootstrap/reset
  及 worker 不能各自建立第二套连接生命周期；错误 backend transaction 配对被拒绝；受支持生产
  平台、本地文件系统、稳定身份与 identity-following OS lock 语义的正向 allowlist、
  unknown/unsupported/capability-unverifiable fail-closed 行为及官方容器本地卷策略已冻结并通过
  测试；数据库由 no-follow 流程创建/打开，已有 DB/WAL/SHM/旁路 lock 文件在支持可靠
  link-count 的冻结平台上均拒绝 `!= 1`，锁定身份在创建后及 pool/migration 前均与 no-follow
  重解析路径一致；identity lock 持续到 pool/连接/checkpoint 关闭后，同一稳定身份的受支持 alias
  不能启动第二个 runtime；lifecycle/control/log 每个物理连接（含强制重开和 pool 回收替换）都在可用
  前以及 pool checkout/recycle 后再次交付前设置并读回验证 `synchronous=FULL`、
  `foreign_keys=ON` 与统一 busy policy，且观察到 WAL；
  第二进程、身份或锁能力不可验证、弱权限、任一连接 pragma/busy policy 不匹配均 fail closed。
- **必测**：严格 URL parse、单次 percent-decoding/规范化、无 query/authority/userinfo/
  fragment、feature/password_file 矩阵；no-follow 原子创建/打开、DB/WAL/SHM/旁路 lock
  link-count、稳定身份记录与锁后 path/identity 复验。两个真实进程分别覆盖同路径、percent-encoded
  alias、hard-link alias 和 bind/mount alias 的 identity exclusion/失败；只有在 CI 确实缺少
  构造某个 alias case 所需 capability 时，才以确定性的低层 identity/descriptor-lock 跨进程测试
  替代该 case，并明确记录 capability 判定，禁止 silent skip。测试结论只约束冻结的受支持平台与
  文件系统。分别枚举
  lifecycle、control pool、log
  pool 的初始连接，强制断开/重开、把被故意改变 pragma 的连接回收到 pool 后再次 checkout，以及
  回收替换物理连接，验证每次都重新 set + read-back
  `synchronous=FULL`/`foreign_keys=ON`/busy timeout 并检查 WAL，注入每一项不匹配时连接不得进入
  pool；两个逻辑 pool 的共享 writer 公平性、owner 只能创建一次、CLI/serve 生命周期复用、锁
  的全生命周期保持与 pool/checkpoint 后释放、WAL checkpoint、正向
  local filesystem allowlist 与 known-remote/unsupported/unknown 分类、官方容器卷、权限、
  migration dispatch、pool/writer metrics、PostgreSQL 回归和 MSRV。
- **风险/工作量**：高，9–13 日。风险集中于 SQLx transaction 生命周期、跨 pool 锁公平性和
  冻结平台/文件系统上的 no-follow 文件生命周期、稳定身份/alias 锁语义及文件权限；不把该保证
  扩展到未验证组合。

### M3 / I-03：控制面生命周期与读取

- **范围**：把现有 SQLite runtime reader 接入 `ControlPlaneRepository` facade；补齐 Console
  所需控制面 list/detail、ETag/version、system setting 和 MCP 的只读能力；实现启动时
  system-probe identity 的创建/读取；统一一致读快照与 reload 语义。本里程碑不实现
  `/system/load`。
- **依赖**：M2。
- **关键文件/模块**：`src/persistence/sqlite/runtime.rs`、新增 SQLite control-plane/read
  模块、`src/persistence/postgres/mod.rs`、`src/runtime_config/`、snapshot reload worker。
- **进入条件**：SQLite pool/transaction 可由 facade 使用；runtime record contract 已冻结。
- **退出条件**：相同 fixture 在两个后端产生相同完整 `RuntimeConfigRecords`、compiled snapshot、
  list/detail DTO、排序和 not-found/ETag 结果；读取过程中不能观察半次管理写入。
- **必测**：共享 runtime repository contract、malformed storage fail-closed、并发 reader/writer
  snapshot、reload/version、system-probe identity 首次创建/重复启动读取、MCP feature on/off、
  PostgreSQL 回归。
- **风险/工作量**：中，7–10 日。主要风险是 JSON/TEXT 解码和分页/排序细节漂移。

### M4 / I-04：身份与访问写入

- **范围**：把已有约 24 个 SQLite auth/account/registration 方法正式置于
  `AuthRepository` facade；补齐 users、user groups、API keys、API key policies 及其 ownership、
  secret 一次返回、审计和 ETag mutation。
- **依赖**：M3。
- **关键文件/模块**：`src/persistence/sqlite/auth.rs`、
  `src/persistence/sqlite/auth/`、新增 SQLite identity/access writer、PostgreSQL auth/control
  实现和 Console application service。
- **进入条件**：一致读、管理写事务和 facade dispatch 已通过；现有 SQLite auth suites 全绿。
- **退出条件**：身份、Session、注册、邀请、用户组、Key/Policy 的状态机、并发冲突、ownership 和
  audit 在两个后端逐项等价；应用调用方不出现 backend 分支。
- **必测**：共享 auth/account/registration/access contract；refresh replay、临时密码并发、
  invitation 单次消费、API Key secret 一次返回、ETag 冲突、audit rollback/redaction、Unicode
  canonical email。
- **风险/工作量**：中，6–9 日。主要风险是唯一约束、canonical email 和一次性 secret 的差异。

### M5 / I-05：路由、系统与 MCP 写入

- **范围**：实现 models、model rules、channel groups、普通 channels、proxies、templates、
  system settings、MCP servers 的 SQLite create/update/delete/batch；显式完成跨表约束、候选
  compile、audit、commit、publish；实现 scheduled channel probes，以及自动 disable/recovery
  的持久化和 snapshot publish 语义。
- **依赖**：M4。
- **关键文件/模块**：SQLite control-plane writers、`src/domain/system_settings.rs`、
  `src/domain/mcp.rs`、`src/runtime_config/`、Console control-plane application service。
- **进入条件**：identity/access 和管理事务 contract 完成；候选 snapshot 可在 transaction 内读。
- **退出条件**：全部非 Codex 控制面 API 在 SQLite 上与 PostgreSQL 等价；格式一致性、managed
  channel 保护、MCP registry 和 system setting feature gate 均不弱化；scheduled probe 的自动
  disable/recovery 能持久化并按固定顺序发布；事务内完整候选编译继续作为有界例外，可支持配置
  的记录数/序列化字节上限、writer queue wait、writer lock-held duration、transaction duration
  telemetry 和各自的数值 pass/fail 阈值已冻结；publish 顺序被测试固定。
- **必测**：每类 CRUD/batch 的共享 contract、constraint/audit/rollback、并发 ETag、编译失败不
  发布、commit 失败不发布、候选恰好在配置大小边界内/外、writer queue wait/lock-held/
  transaction duration telemetry 与阈值、scheduled probe disable/recovery 的 commit/publish/
  restart、MCP feature on/off 和 runtime snapshot 原子替换。
- **风险/工作量**：高，9–13 日。风险是 PostgreSQL trigger/constraint 的 SQLite 显式重建和
  mutation 数量。

### M6 / I-06：耐久请求日志与结算

- **范围**：实现 SQLite ingress batch、final projection、backlog/health、settlement 和
  `RequestLogRepository` dispatch；保留 spool 协议和两个逻辑 pool，加入公平 batch 限制和精确
  Decimal Rust fold；拥有 request-log backlog/health，并把它与 M2 的 pool/writer 指标接入
  `/system/load` 所用的 `SystemMetrics`。
- **依赖**：M2，可与 M3–M5 和 M8 并行；后续 I-10 仍必须等待 M7。
- **关键文件/模块**：`src/workers/durable_request_log.rs`、
  `src/request_log_spool.rs`、SQLite request-log repository、settlement worker、load metrics。
- **进入条件**：SQLite lifecycle、writer coordinator、typed errors 和 fault injection harness
  可用。
- **退出条件**：所有 crash matrix cut point 都不丢已承诺记录、不重复 final/settlement；spool、
  ingress、final、余额/API Key 额度可在重启后收敛；request-log backlog/health 与 M2
  pool/writer 指标通过 `SystemMetrics` 进入 `/system/load`；任何金额路径均无
  REAL/f64/SUM(TEXT)。
- **必测**：阶段级 fault injection、kill/restart、重复 UUID、malformed journal 隔离、磁盘只读/
  满、busy timeout、batch 公平性、backlog/health 和 `/system/load` 集成、settlement 并发与
  exact Decimal fixture。
- **风险/工作量**：高，12–17 日。这是数据丢失和重复扣费风险最高的里程碑之一。

### M7 / I-07：查询、统计与排行榜

- **范围**：实现 request-log list/detail、个人 usage、渠道状态、花费统计、Codex 周期花费、
  leaderboard；在 Rust 中实现 SQLite 缺失的时间/percentile/Decimal 聚合并限制资源。不实现
  `/system/load`、pool/writer metrics 或 backlog/health。
- **依赖**：M6。
- **关键文件/模块**：SQLite query/statistics/leaderboard repository、statistics application
  service、leaderboard worker、`rust_decimal` 与时区聚合 helper。
- **进入条件**：SQLite final logs 和 settlement 稳定；canonical statistics fixture 完整。
- **退出条件**：两个后端逐字段返回相同统计 DTO、空桶、P90/P50、成功率、成本和榜单；大窗口查询
  有稳定内存/时长边界；排行榜失败保留旧快照。
- **必测**：跨后端 golden/contract、DST 与 `Asia/Shanghai` 周月边界、UTC 365 日、无数据、
  重叠 Codex 周期、精确小数、稳定 tie-break、refresh crash/重启、最大允许窗口和大 fixture。
- **风险/工作量**：高，10–15 日。风险是 SQL 方言、percentile、时区和大数据量性能差异。

### M8 / I-08：Codex 跨 HTTP 事务重构

- **范围**：先在现有 PostgreSQL 生产路径新增 durable refresh claim/generation/dispatched/CAS
  和 durable idempotent reset operation；为 reset recovery 增加 stable
  `redeem_request_id`、single-active attempt claim/generation/lease、terminal CAS 与 immutable
  result；实现 quota attempt pre-dispatch registration、fence generation、dispatch/terminal
  boundary、fence-owned 单调 anchor generation、每 generation 单 active claim/lease、anchor
  timeout/crash replacement、stale generation rejection、原子 drain cutoff seal 和不依赖
  `checked_at` 的 drain。
  实现 pending/dispatched/recovering/unknown unresolved reset 或 active/draining quota fence 对
  credential destructive mutation 和 batch 的 typed conflict fence，并保留 tombstone 后的
  operation/event/audit。新增 OpenAPI-first、admin-only 的 unknown reset status/list/detail 和
  versioned reconciliation endpoint（ETag/`If-Match`、required `Idempotency-Key`、
  `confirmed_reset | confirmed_not_reset`、有界非 secret evidence/reason、immutable audit），并
  让两种 resolution 都创建/重启 anchor generation；rotating refresh unknown 仍只能
  reauthorization。只在 fence 外保留现有 stale `checked_at`/version guard。Console reset
  endpoint 同样以 OpenAPI-first 方式增加
  required `Idempotency-Key`、`202` pending/retry 和 `409` intent/mutation conflict；同步
  terminal `200` response shape 尽量保持不变。更新生成类型、Console client/UI，使每次用户意图
  只生成一个 key 并在 retry/resume 中复用，并实现 operator reconciliation list/detail/form 和
  runbook。删除所有跨 provider HTTP 的数据库 transaction。该
  PR 同时追加语义对应的 PostgreSQL 与 SQLite migration 和 schema parity 测试，但不提前接入
  SQLite Codex repository。
- **依赖**：M2；不等待 M5/M7，但它产出的 OpenAPI、durable reset 和 quota fence contract 是
  I-09/I-10 的前置，不能延后到 M9 补做。
- **关键文件/模块**：`docs/openapi/console-v1.yaml`、
  `web/console/src/api/generated/console-v1.d.ts`、`web/console/src/api/client.ts`、
  `web/console/src/features/admin/providers/codex-oauth/`、`src/http/console.rs`、
  `src/application/codex/`、`src/persistence/postgres/codex.rs`、Codex migrations、
  `tests/console_spec_integration.rs`、Console component/E2E 与 mock provider tests、Console
  operator runbook。
- **进入条件**：事务意图和 typed conflict 已可表达；旧 terminal response、quota version guard、
  window classification 和 Console reset 行为有完整基准测试；provider 对稳定
  `redeem_request_id` 的 retry/`already_redeemed` 语义已版本固定并有 mock contract；OpenAPI
  生成/漂移 gate 可运行。
- **退出条件**：代码审计确认 provider HTTP 时无数据库 transaction；并发实例只消费一次有效
  generation；仅 pre-dispatch 崩溃可安全恢复，任何已 dispatched/abandoned 的 rotating refresh
  都持久 fail safe 到 `refresh_outcome_unknown` / `reauth_required`，且不自动重试旧 token；
  reset 的 actor+credential+Console key 只绑定同一 durable `redeem_request_id`，相同 intent
  可恢复/重放 terminal 结果而不同 intent conflict；每次 recovery 只有一个 durable active
  attempt claim/generation，terminal finalization 只从 expected active+dispatched attempt CAS，
  operation/result immutable，operation/event/audit/fence terminal marker 原子且各自按 operation
  ID 唯一。旧 attempt completion 只能读取胜出的 terminal result。required Header、
  `200`/`202`/`409` 已先写入
  OpenAPI，生成类型、client、UI 和 Console handler/spec tests 同步。admin-only reset
  operation list/detail 与 resolution contract 也已先进入 OpenAPI：强 ETag/`If-Match`、required
  resolution `Idempotency-Key`、两种 outcome、有界结构化非 secret evidence/reason 和明确
  conflict/stale response 已实现；相同 resolution key 的 response-loss replay 返回相同结果，
  并发 resolver 只有一个 CAS 胜者。`confirmed_reset` 只写一个 manual event，
  `confirmed_not_reset` 不写 event，两者都留下 immutable redacted audit 并创建/重启 anchor
  generation；Console 生成类型/client/admin UI、component/spec/E2E 和禁止 DB 手改的 runbook
  同步，refresh unknown 不进入该 resolver。每个 quota HTTP attempt
  dispatch 前均已注册 version/state/fence generation；reset 创建已快照 in-flight attempt，跨
  dispatch/terminal boundary 的响应只可标为 ambiguous/stale；terminal 后 fence-owned
  `anchor_generation` 单调递增，每 generation 只有一个 active claim/lease，timeout/crash/failure
  只 expire/fail 当代且 recovery worker CAS 创建下一代。只有最新 generation durable success
  以及后续 observation 可按 durable 顺序推进；旧代 response 一律拒绝。`active -> draining` CAS
  已原子捕获 cutoff 并阻止新 attempt 加入，全部 cutoff 内 pre-boundary attempt 已
  completed/expired 并分类且胜出 anchor/ordered post-terminal observations 已持久化后才能
  drain/close fence；provider 不可用期间 fence visible/alerted 且不静默打开，恢复后下一代 anchor
  能最终推进，close 后旧 generation 响应仍被拒绝。
  任何实例都不能依靠 `checked_at` 提前写
  `openai_official`。所有 destructive credential mutation/batch 在 unresolved reset 或
  active/draining quota fence 下 typed conflict，unknown 必须显式 reconciliation；event/audit 在
  后续 tombstone 后仍可查。两个 schema 都具备后续 SQLite adapter 所需的等价 operation/attempt
  claim、quota attempt/observation、fence generation/boundary/cutoff、anchor 和 drain cursor
  事实。
- **必测**：refresh claim commit 前后、dispatched 标记 commit 前后、标记后但实际 send 前、
  provider 已接收/轮换后、响应收到但 CAS 前、CAS commit 前后的 kill/restart cut point；lease
  expiry/abandon、双 refresh、stale generation、rotating token、401 trigger、unknown/reauth
  只能 reauthorization；缺失/非法 key、同 key 同 intent 并发/timeout/restart、同 key 换
  actor/credential/action conflict、新用户意图新 key、UI retry 复用 key、pending `202` resume、
  terminal DB commit 后 HTTP response 丢失再以同 key 请求并得到同一结果且 provider 只调用一次；
  reset attempt claim lease 接管、两个 recovery worker 竞争、reset success 与同
  `redeem_request_id` 的 `already_redeemed` 响应竞争，以及旧 generation 在新 generation terminal
  后迟到，均只能得到同一 immutable result 且 operation/event/audit/fence marker 各一份；正常
  `dispatched` operation 的 provider completion 必须可 CAS terminal，而 reset/quota 的
  pre-dispatch completion 均必须拒绝。operator reconciliation 另测 user/未认证拒绝、按
  credential/status list 与 operation detail、ETag、reason/evidence bounds 与 secret-shaped
  input 拒绝、`confirmed_reset` event 恰一份、`confirmed_not_reset` 零 event、两者 immutable
  redacted audit；resolution commit 后 HTTP response loss 以相同 key 重试、两个 admin 以相同/
  不同 key 和 ETag 并发、stale/conflicting transition，均只能有一个 resolution/terminal
  side-effect 和同一 replay result。Console generated types/client/list/detail/form 的
  spec/component/E2E 同步覆盖两种 outcome 和 validation，refresh unknown 对该 endpoint 必须
  typed reject。quota
  必须精确覆盖“request-start 在 reset 前而 provider-process 在 reset 后”的 attempt、terminal
  marker 前注册而后完成、fresh post-terminal anchor、anchor claim timeout、dispatch 前后 worker
  crash、同 generation 两个 recovery worker 竞争、旧 anchor generation response 在新 generation
  创建前后迟到并被拒绝、连续 provider unavailable 产生 visible/alerted failed generations 且
  fence 始终 active、provider 恢复后下一 generation 成功并最终 closure、anchor 后 single-flight
  顺序、pre-boundary expiry、expired 后但 drain/close 前的 late response rejection、quota registration 与
  `active -> draining` seal/close 竞争，以及 fence close 后旧响应迟到并被 generation 拒绝；另测
  successful/manual 与 failed terminal 两条 drain、drain 中 crash/restart 和仅 fence 外 stale
  `checked_at`/version guard。
  credential delete/tombstone、token/identity reimport、connector-pool reassignment 和混合 batch
  分别与 pending/dispatched/recovering/unknown reset 及 successful/failed terminal 后仍
  active/draining 的 quota fence 竞争，均须 typed conflict 且整批无部分写；unknown 经显式
  operator resolution 且 fence drain/close 后才可继续，event/audit 在最终 tombstone 后仍存在。M8 在
  PostgreSQL 上跑真实双 application/process instance；共享 contract 同时固定 SQLite I-09/I-10
  必须复现的调度。另跑 PostgreSQL 全回归、OpenAPI/generated drift、Console spec/component/E2E
  及 PostgreSQL/SQLite migration upgrade/schema parity。mock provider 必须能分别阻塞 request
  start、provider process、response delivery 和 terminal commit，并区分“请求未发送”、
  “provider 已执行但响应未持久化”和“terminal 已提交但 Console HTTP response 丢失”，还必须可
  连续返回 quota unavailable 后恢复，以验证 anchor generation replacement 与 eventual closure。
- **风险/工作量**：高，15–22 日。风险是 OAuth rotating token、reset-credit 不可逆副作用、
  recovery claim/terminal CAS、credential mutation fence、两套 API 幂等生命周期、admin
  reconciliation 误判，以及 quota attempt/fence/anchor replacement backlog/order。

### M9 / I-09、I-10：SQLite Codex 完整实现

- **范围**：
  - **I-09**：OAuth flow、credential import/list/update/delete/export、pool 与 Responses/Images
    managed-channel 显式投影、runtime credential snapshot；在 delete/tombstone、token/identity
    replacement/reimport、connector-pool reassignment 和相关 batch 上实现 M8 定义的 unresolved
    reset 或 active/draining quota fence typed conflict，整批原子拒绝，并保证 eventual
    tombstone 不级联删除 reset operation/event/audit；
  - **I-10**：refresh claim/generation/dispatched/CAS、expired/abandoned dispatched work 到
    durable `refresh_outcome_unknown` / `reauth_required` 的转换、maintenance、quota
    attempt pre-dispatch registration、observation/fence generation/boundary/cutoff、单调
    post-terminal anchor generation、每代 single-active claim/lease、失败代 replacement 与
    ordered drain、window/history、reset actor+credential+
    `Idempotency-Key` operation、stable `redeem_request_id`、single-active recovery
    claim/generation/lease、terminal CAS/immutable result；完整镜像 I-08 的 admin-only
    OpenAPI reconciliation persistence/state behavior（resolution idempotency、If-Match/CAS、两种
    outcome、event/audit/anchor 语义），rotating refresh unknown 仍走 reauthorization；
    pending/terminal visibility、周期成本接缝和全部恢复路径。
- **依赖**：I-09 同时依赖 M5 和 M8；I-10 同时依赖 I-09 和 M7。
- **关键文件/模块**：新增 `src/persistence/sqlite/codex/`、Codex facade dispatch、
  SQLite forward migrations、credential runtime/worker。
- **进入条件**：开始 I-09 前，M5 控制面 publish/probe 语义完成且 PostgreSQL 不再跨 HTTP 持
  transaction，projection invariants 有共享 contract；开始 I-10 前，I-09 与 M7 周期成本/统计
  接缝均完成。
- **退出条件**：SQLite 覆盖 Codex 全部管理、运行时、Responses/Images 投影、refresh、quota
  attempt/observation/fence/anchor generation replacement/drain、reset API operation/recovery
  claim/terminal immutable result、admin-only unknown reconciliation persistence/state、
  destructive mutation fence、tombstone 和 audit；无 trigger
  依赖；同一 OpenAPI/Console 客户端无需后端分支，mock provider 全链路与 PostgreSQL 等价。
- **必测**：I-09 测身份匹配、双 projection、格式隔离、tombstone、OAuth flow 单次完成和审计；
  同时把 delete/reimport/pool reassignment/混合 batch 与已有 unresolved reset 或
  active/draining quota fence facts 交错，验证 typed conflict、整批 rollback 及 tombstone 后历史
  保留。I-10 重跑全部 M8 并发/崩溃 cut point，
  特别验证 dispatched 后 provider 已轮换但 CAS 前崩溃会进入 unknown/reauth 且不重试旧 token；
  另测 reset 同 Console key/同 provider request ID 幂等恢复和不同 intent conflict、recovery
  claim lease/generation 接管、reset success 对 `already_redeemed` 竞争、stale completion 读取
  immutable winner、terminal commit 后 response-loss replay；unknown 显式 reconciliation 对
  `confirmed_reset`/`confirmed_not_reset` 的 event 差异、resolution response-loss 同 key replay、
  两个 resolver CAS 竞争、stale/conflicting transition、reason/evidence bound、audit redaction 和
  refresh-unknown rejection 必须与 PostgreSQL 逐状态等价；resolution 后若
  quota fence 仍 active/draining，mutation 必须继续 conflict，只有 fence drain/close 后才解锁；
  并把 delete/reimport/pool reassignment/混合 batch 与 live
  pending/dispatched/recovering/unknown transition、terminal 后 active/draining quota fence 逐一
  竞态，验证无 TOCTOU 或部分写；reset/quota pre-dispatch completion 均必须拒绝。quota 必须复现
  request-start-before-reset/provider-process-after-reset、
  terminal 前注册后完成、fresh post-terminal anchor、anchor timeout/crash、同代 competing
  workers、下一代 CAS replacement、旧代 stale response rejection、provider unavailable 时
  visible/alerted active fence 与恢复后的 eventual closure、pre-boundary expiry、expiry 后且
  drain/close 前 late-response rejection、ordered post-terminal observations、quota registration
  对 drain cutoff seal/close race、fence close 前 drain cursor crash recovery 及 late-after-close
  generation rejection；
  另测成功/失败 terminal 分类、仅 fence 外 stale `checked_at` guard、周期金额精确聚合和重启恢复。
  共享跨后端 suite 必须在 PostgreSQL 双实例和 SQLite 同一 `DatabaseRuntime` 下两个独立
  application/worker 实例的交错调度中通过；SQLite 测试不绕过单 Gateway 进程生产拓扑去启动第二个
  持锁进程。
- **风险/工作量**：高，合计 15–23 日。拆成两 PR 是风险隔离，不允许 I-09 被单独当作可用 Codex。

### M10 / I-11：全栈 facade/runtime 收敛

- **范围**：移除 application、HTTP、MCP、worker 和 startup 中残余 PostgreSQL concrete
  依赖；所有服务只接收中立 facade；在内部测试入口运行完整 SQLite Gateway。正常配置仍不开放
  SQLite URL。签入可复现的 SQLite 资格 profile，明确数值 fixture 规模、负载、时长、并发工作和
  每项通过阈值，供 M11 原样执行。
- **依赖**：M5、M7、M9 全部完成。
- **关键文件/模块**：`src/main.rs`、`src/application/`、`src/http/`、`src/mcp/`、
  `src/workers/`、`src/runtime_config/`、全栈 integration harness。
- **进入条件**：三条并行轨各自的跨后端 contract 全绿，所有 TODO/临时 fallback 有清单。
- **退出条件**：非 persistence 模块没有 backend branch 或 concrete SQLx backend；唯一
  `DatabaseRuntime` 是 serve/CLI/worker 的生命周期入口；SQLite 全栈覆盖 bootstrap、Console、
  转发、日志、结算、统计、MCP、Codex、scheduled probe 自动 disable/recovery 和重启；M5 的
  配置大小及 writer queue wait/lock-held/transaction duration telemetry 与阈值已纳入签入的
  数值资格 profile；配置激活门仍关闭。
- **必测**：完整 feature/MSRV 矩阵、全栈 SQLite 场景、scheduled probe 自动 disable/recovery、
  PostgreSQL 全套回归、静态 grep/架构测试、资格 profile 的可复现性和 graceful/forced shutdown。
  若本里程碑实际改动任何转发路径，还必须在明确授权后运行付费真实上游 smoke；没有授权或凭据时
  本里程碑保持阻塞，mock 不可替代。
- **风险/工作量**：高，8–12 日。风险是隐藏的 concrete pool/transaction 和跨模块启动顺序。

### M11 / I-12：生产启用与运维

- **范围**：在同一个 I-12 PR 内先保持激活关闭，运行并记录预激活 gate；通过评审后追加最终最小
  激活改动，该改动才开放文件型 SQLite URL，然后原样重跑 gate。官方 Docker/Release artifact
  编译 `sqlite-backend`，Cargo 默认可保持 PostgreSQL-only；同步两份配置模板、单独的 SQLite
  部署示例、用户运维文档、容器文件权限准备、监控、一致备份/恢复、容量、升级和故障处理说明。
  PostgreSQL 保持默认，公开声明仍须等待激活后 gate 通过。
- **依赖**：M10。
- **关键文件/模块**：runtime config、配置模板、container entrypoint、用户部署/生产配置文档、
  release verification 和运维测试。具体改动须遵循届时的配置/部署同步矩阵。
- **进入条件**：M10 全绿且没有阻断级数据完整性、性能或安全问题；支持拓扑文字已冻结。
- **退出条件**：预激活与激活后两次 gate 都通过；官方 artifact 可显式选择安全的本地 SQLite
  文件而 Cargo 默认仍是 PostgreSQL；PostgreSQL 官方部署与单独 SQLite 示例不混用拓扑；安装、
  升级、磁盘满、权限修复和回滚 runbook 经演练；在同步 spool/manifest 的 backup barrier 内使用
  SQLite backup API 或协调 checkpoint 生成一致数据库备份，备份加密、受访问控制并复制到受保护
  的异机/对象存储；数据库、spool、spool checkpoint/manifest 的一致恢复和幂等重放 drill 通过；
  所有文档只宣称单进程本机 live 文件拓扑，并发布已实测的支持容量。
- **必测**：配置正反例和首发零 query parameter、feature-disabled 错误、官方 Docker/Release
  feature 检查、PostgreSQL 与单独 SQLite 部署示例、容器 UID/GID 与正向 local filesystem
  allowlist、实际文件/WAL 权限、backup API/协调 checkpoint、加密与 off-host copy、数据库+spool
  coherent restore、migration upgrade、磁盘满/损坏诊断、最终全栈；在明确授权后原样运行并通过
  M10 签入的持续资格 profile。未获持续性能运行授权时 M11 保持阻塞。
- **风险/工作量**：高，6–8 日。风险是过早宣传、错误复制 live DB 文件和容器卷权限。

## 7. 测试总策略

### 7.1 跨后端 contract

- 为 `AuthRepository`、`ControlPlaneRepository`、`RequestLogRepository` 建立共享黑盒 suite；
  同一测试函数/宏分别创建 PostgreSQL 与 SQLite fixture，调用相同 facade 公共 API。
- 比较 canonical DTO、排序、分页、错误类别、事务回滚、ETag、audit、secret redaction 和最终
  数据库状态，不比较 SQL 文本或后端原始错误。
- 所有金额 fixture 包含 8 位 scale、舍入边界、大值、负余额、重复结算和可揭露浮点误差的值。
- schema parity 测试继续存在，但列数相同不能替代行为 contract。

### 7.2 feature 与 MSRV 矩阵

| 构建/测试模式 | 每个相关 PR | M10/M11 最终门禁 |
| --- | --- | --- |
| 默认 feature（PostgreSQL） | fmt、clippy、workspace tests | 必须 |
| `sqlite-backend` | clippy、unit、全部 SQLite integration | 必须 |
| `mcp-server` | 受影响时 lib/integration | 必须 |
| `sqlite-backend,mcp-server` | M3/M5/M9 起受影响时 | 必须 |
| `embedded-console-ui` 与组合编译 | 接缝变化时 | 必须 |
| Rust 1.92、`--locked`、workspace/all-targets | 至少 check；契约变化时 tests | check + tests 必须 |
| 默认工具链 1.97.1 | 全部常规门禁 | 必须 |

PostgreSQL contract 测试使用隔离数据库；SQLite 每个测试使用独立本地临时目录，不共享 DB、spool
或 lock。任何里程碑只要修改转发路径，就必须遵循 `AGENTS.md`：先取得明确授权，再运行付费真实
上游 smoke。授权、凭据或上游条件不可用时，受影响里程碑保持 `[!] 阻塞`；确定性 mock 是必要覆盖，
但不能替代真实上游 smoke。未修改转发路径的纯 persistence 工作不因此触发付费调用。

### 7.3 fault injection 与 migration upgrade

- 在 transaction begin/write/commit、spool checkpoint、projection delete、settlement、Codex
  refresh claim/dispatched/provider/CAS、reset operation create/dispatched/provider/terminal
  attempt claim/lease handoff/provider/terminal CAS/Console HTTP response、quota attempt
  register/dispatched/provider process/response persist、fence generation attach、reset
  dispatch/terminal boundary、anchor generation claim/register/dispatched/result/lease expiry/
  failed-generation replacement、`active -> draining` cutoff seal、
  concurrent registration、drain classify/apply/cursor/fence close、credential destructive mutation
  check/write、operator resolution CAS/event/audit/anchor restart/HTTP response、leaderboard swap 和
  WAL checkpoint 前后设置确定性
  failpoint。
- reset 必须有独立的 terminal DB commit → HTTP response loss → repeat same
  `Idempotency-Key` failpoint：重试返回同一 terminal response，数据库仍只有一个 intent/
  `redeem_request_id`，mock provider 调用计数不增加。另以 barrier 精确制造旧 reset attempt
  response 与新 claim generation 的 success/`already_redeemed` 竞争；胜者 terminal commit 后，
  stale completion 只能读取同一 immutable result，operation/event/audit/fence marker 计数始终为
  一。quota mock 必须把 request start 与 provider process 分开阻塞，精确制造
  request-start-before-reset/provider-process-after-reset，以及 fence close 后旧 generation 响应
  才返回；另制造 attempt `expired` 后、drain/close 前才返回。anchor matrix 还必须在 claim 后、
  dispatch marker 后、provider success 后但 observation commit 前 kill worker，令 lease
  timeout/请求失败把当代标为 `expired`/`failed`，再让两个 recovery worker 竞争下一
  `anchor_generation`。每代只能一个 active claim；旧代 response 在新代创建前后到达都必须拒绝。
  连续 provider unavailable 时每代失败事实、当前 generation 和 active/alerted fence 必须持久
  可见；恢复后新一代成功，ordered drain 最终关闭 fence。第一种只能 `ambiguous`，其他 stale/late
  response 必须
  拒绝且不能改写 immutable attempt/classification/cursor。terminal event commit 后任一 cut point
  崩溃都必须从 durable attempt/observation/cursor 继续，不能产生重复 period、遗漏 observation、
  接受非最新 anchor 或暂时落库错误的 `openai_official`。quota registration 与 cutoff seal/close
  竞态必须证明 attempt 要么被纳入 durable cutoff 并完整 drain，要么等待 close 后按 fence 外新
  attempt 注册，不能落入两者之间。
- 注入 provider completion 到尚为 `registered` 的 reset/quota attempt；两者都必须拒绝且不能生成
  terminal operation、observation、event/audit 或 fence marker。只有 durable `dispatched` attempt
  能接受 provider-derived completion。
- 对每个 unresolved reset 状态在 destructive mutation 的 fence check 前后、batch 第一成员写入前
  后和 commit 前注入故障/竞争；delete、reimport、pool reassignment 及混合 batch 均必须 typed
  conflict、全回滚且保留 operation/event/audit；同一矩阵也覆盖 terminal 后 quota fence 仍
  active/draining。unknown 只能由显式 operator resolution transaction 改变，且 resolution 后仍须
  等 quota fence drain/close；重启或 lease 到期不能解除 destructive fence。
- 对 admin reconciliation 在 `If-Match` 检查、resolution idempotency insert、terminal/result、
  manual event（仅 `confirmed_reset`）、audit、anchor-generation restart 和 commit 前后逐点注入
  故障；commit 后丢失 HTTP response 再用同 key 请求必须返回同一结果。两个 admin/worker 以相同
  或不同 key、相同或 stale ETag 竞争时只有一个 CAS 胜者；败者不得写第二个 event/audit/anchor。
  `confirmed_not_reset` 始终无 reset event，但必须和 `confirmed_reset` 一样启动/重启 anchor。
  audit/response/日志不含 evidence 边界外内容或 raw provider secret；普通 user、refresh-unknown
  operation 与直接数据库编辑路径均不能通过该状态机。
- 除普通 error injection 外，对关键 cut point 执行子进程 kill/restart，验证磁盘事实，而不只验证
  内存 mock。M8 先在 PostgreSQL 双实例验证；I-10 用同一 suite 在 SQLite 单进程拓扑内两个独立
  application/worker 实例交错执行，并对两个后端分别执行 crash/restart。
- M2 对路径旁路创建锁取得前后、数据库 no-follow create/open、identity 记录、descriptor/identity
  OS lock 取得、锁后 path/identity/link-count 复验、WAL/SHM 创建后复验、pool/migration 启动前
  和 pool/checkpoint 关闭后的锁释放分别设置 failpoint。两个子进程必须覆盖同路径、编码 alias、
  hard-link alias 与 bind/mount alias；若 CI capability 无法构造某类 alias，必须运行能证明
  相同稳定身份共享同一 descriptor/identity lock 域的确定性低层跨进程测试并报告该 capability，
  不能 silent skip 或把未测平台列入支持矩阵。
- 覆盖 busy timeout、长 reader、writer 饥饿、磁盘满、只读目录、WAL 残留、截断 journal、
  malformed TEXT/JSON/time、连接中断和 shutdown deadline。
- PostgreSQL 从完整 migration 历史和受支持旧版本 fixture 升级；SQLite 从 `0001` 及之后每个
  已发布 SQLite 版本 fixture 升级。验证中断后原子回滚或可重复继续、schema 语义、pragma 和数据
  保真。
- M11 首次生产启用前保存可复现的首发 SQLite fixture；此后 migration 只能前向追加。

### 7.4 全栈 SQLite 场景

M10/M11 至少自动化以下单文件场景：

1. migration、bootstrap admin、login/refresh、邀请、Profile/密码和 Session；
2. 完整 Console 控制面 CRUD、API Key 一次性 secret、snapshot publish/reload；
3. Chat Completions、Responses HTTP/WebSocket、Images、standalone search 的确定性 mock 转发；
4. MCP Search/Images（启用 feature 时）、系统设置变更、scheduled channel probe 自动
   disable/recovery 的持久化与 publish；
5. Codex OAuth/import、双 projection、refresh 的 unknown/reauth fail-safe；Console reset
   required `Idempotency-Key`、同用户意图 pending `202`/retry/restart 复用、terminal response-loss
   replay、single-active recovery attempt generation、success/`already_redeemed` race、stale
   completion 读取 immutable winner、不同 intent `409` 和新意图新 key；pending/dispatched/
   recovering/unknown reset 下 delete/reimport/pool reassignment/batch typed conflict，unknown 经
   admin-only list/detail + versioned operator reconciliation 解决：两种 outcome、ETag/
   `If-Match`、resolution `Idempotency-Key` response-loss replay、并发 resolver、event 差异、audit
   redaction 与 Console UI 均端到端验证，rotating refresh unknown 仍 reauthorization；resolution
    后 quota fence drain/close 才解锁，terminal 后 active/draining fence 仍拒绝 mutation，eventual
   tombstone 保留 reset event/audit；quota attempt dispatch 前注册，
   精确交错 request-start-before-reset/provider-process-after-reset、terminal 前注册后完成、fresh
   post-terminal anchor、anchor timeout/crash 与 competing replacement workers、stale old-anchor
   response、provider unavailable 时 visible/alerted fence 和恢复后的 eventual closure、
   expired-before-drain late response、registration-vs-cutoff-seal 和 late-after-close，成功
   terminal 仅以最新成功 anchor 及后续 durable order 分类 `manual`、失败 terminal 不带 event，
   fence 内不按 `checked_at` drain，随后删除和重启；
6. spool backlog、ingress、final、settlement、个人/系统统计和排行榜端到端收敛；
7. 优雅关闭、强制终止、WAL/spool 恢复，以及一致数据库备份与 spool/checkpoint manifest 的
   coherent restore 后再次启动；
8. 第二进程通过同路径、percent-encoded alias、hard-link alias 和 CI capability 允许的
   bind/mount alias 竞争同一锁定身份；不具备 alias 构造 capability 时运行确定性低层
   identity/lock 测试而非 silent skip。另覆盖 DB/WAL/SHM/旁路 lock 的异常 link count、锁后身份
   或路径变化、错误权限、known-remote/unsupported/unknown 文件系统分类、稳定身份/锁能力不可
   验证、磁盘满和损坏文件的 fail-closed 诊断；只对冻结支持矩阵作出保证。

### 7.5 可复现容量资格

M10 必须签入一个版本化、可单命令复现的 SQLite qualification profile，而不是在 M11 临时选择
“看起来足够”的负载。profile 必须用数值固定：

- 数据库记录、runtime config 记录/序列化字节、request-log/backlog 和统计窗口 fixture 规模；
- 转发请求率与日志产生率、持续时间，以及并发 Console mutation/query、ingestion、settlement、
  scheduled probe、Codex refresh/quota/reset、reset recovery claim handoff、unresolved reset
  destructive-mutation conflict、admin reconciliation list/detail/CAS、active fence 期间 quota
  attempt backlog、pre-boundary expiry、anchor generation timeout/replacement、provider unavailable
  后恢复与 terminal drain 工作量；
- 未处理 `busy` 错误（必须为零）、writer queue wait、writer lock-held duration 与 transaction
  duration、Console latency、backlog 最大增长/停止负载后的恢复时间、shutdown drain deadline、
  quota fence backlog/anchor replacement/drain latency、failed generation alert age、provider
  恢复后的 eventual closure deadline、attempt expiry/stale-anchor/late-response rejection、WAL
  峰值/回收值和总磁盘预算的逐项数值 pass/fail 阈值。

profile 必须声明硬件/文件系统基线、预热、采样、重复次数和结果 artifact，失败不能由人工主观豁免。
M11 在预激活 gate 与最小激活改动后都使用同一 profile；最终至少一次获明确授权的持续运行必须全项
通过，并把 profile 对应的支持容量发布到运维文档。任何持续 performance/soak 执行仍须用户明确
授权；未获授权不是“跳过并通过”，而是 M11 阻塞。

## 8. Definition of Done 与非目标

全部条件满足才算计划完成：

- [ ] 11 个里程碑和 12 个实施 PR 均达到各自退出条件。
- [ ] 三个 repository facade 和数据库生命周期完全 enum dispatch；唯一 `DatabaseRuntime`
  创建并关闭两类逻辑 pool，serve/CLI/worker 无独立二次连接，应用层无 backend 分支。
- [ ] SQLite 在冻结拓扑内覆盖 PostgreSQL 的全部现有功能和安全边界，没有 reduced profile。
- [ ] 日志/结算 crash matrix、Codex side-effect matrix（含 dispatched 后 unknown/reauth fail-safe
  与 reset actor+credential+`Idempotency-Key` 到单一 `redeem_request_id`、terminal commit 后
  response-loss replay、single-active recovery claim/generation、terminal CAS/immutable result 和
  success/`already_redeemed`/stale completion race）、quota attempt/fence/anchor/drain、统计 parity
  和 migration upgrade 在两个后端全绿；provider-derived completion 只接受 expected
  active+dispatched attempt，pre-dispatch completion 在两个路径均被拒绝。
- [ ] 每个 quota HTTP attempt 都在 dispatch 前持久注册 attempt/version/state/fence generation；
  request-start-before-reset/provider-process-after-reset 和所有跨 dispatch/terminal boundary 响应
  均只能 ambiguous/stale；fence 拥有单调 `anchor_generation`，每代只允许一个 active
  claim/lease，timeout/crash/failure 只 expire/fail 当代且 competing recovery workers 只有一个
  能 CAS 创建下一代。旧代 response 被拒绝，只有最新代 durable success 可驱动 ordered drain；
  provider unavailable 时 fence visible/alerted 且不静默 open，恢复后最终可 closure。全部
  pre-boundary attempt completed/expired 且持久分类、胜出 anchor 和 ordered post-terminal
  observations 持久化后才 drain；`active -> draining` CAS 原子 seal cutoff 且与 concurrent
  registration 无遗漏，完成后才 close；expired attempt 在 drain/close 前的 late response 与
  late-after-close 均被 CAS/generation 拒绝。successful/manual、failed terminal、anchor
  replacement/eventual closure 与 drain crash/restart 分支已在 PostgreSQL 和 SQLite 的受支持实例
  模型中全绿；仅 fence 外 stale `checked_at`/version guard 无回归。
- [ ] pending/dispatched/recovering/unknown unresolved reset 对 credential delete/tombstone、
  token/identity replacement/reimport、connector-pool reassignment 和包含这些变更的 batch 施加
  typed conflict；successful/failed terminal 后 active/draining quota fence 继续阻挡同类 mutation；
  unknown 只经显式 operator reconciliation 解决且仍等待 fence drain/close，event/audit 在
  eventual tombstone 后保留；I-08、I-09 和 I-10 race/failpoint suite 全绿。
- [ ] Console reset 契约已 OpenAPI-first 更新；required Header、terminal `200`、pending `202`、
  intent conflict `409`、生成前端类型/client/UI 和 Console spec/component/E2E tests 无漂移。
  admin-only reset-operation status/list/detail 与 versioned resolution endpoint 同样
  OpenAPI-first：强 ETag/`If-Match`、required `Idempotency-Key`、`confirmed_reset` /
  `confirmed_not_reset`、有界非 secret evidence/reason、immutable redacted audit、response-loss
  replay、concurrent/stale resolver conflict 和两种 event/anchor 行为在 PostgreSQL/SQLite
  等价；operator runbook 禁止直接 DB 编辑并明确 rotating refresh unknown 只能 reauthorization。
- [ ] 默认、SQLite、MCP 组合和 Rust 1.92 MSRV 矩阵全绿。
- [ ] URL 解码/规范化、零 query parameter、文件系统/稳定身份/identity-following lock 正向
  allowlist、官方容器本地卷、文件权限、link-count、no-follow create/open、锁后 path/identity
  复验、排他锁和 checkpoint 门禁全绿；规范路径和旁路锁没有被当作 hard-link/bind/mount alias
  排除机制。冻结支持矩阵上的两个真实进程对同路径、编码 alias、hard-link alias 以及 capability
  允许的 bind/mount alias 均竞争/失败；CI 无对应 capability 的 case 有确定性低层
  identity/descriptor-lock 测试而非 silent skip。identity lock 只在全部 pool/连接和 checkpoint
  关闭后释放；lifecycle/control/log 每个物理连接（含重开/回收替换）均在首次
  可用前以及 pool checkout/recycle 后再次交付前 set + read-back `synchronous=FULL`、
  `foreign_keys=ON`、统一 busy policy，并验证 WAL；任一不匹配 fail closed。
- [ ] 加密受控的 off-host 备份及数据库+spool coherent restore runbook 经演练，备份凭据和恢复
  密钥风险有明确轮换、最小权限与失效处理。
- [ ] M10 数值 qualification profile 在授权后由 M11 全项通过，并发布支持容量。
- [ ] PostgreSQL 仍为默认，现有 PostgreSQL 行为和部署路径无回归。
- [ ] 官方 Docker/Release artifact 编译 `sqlite-backend`，Cargo 默认可保持 PostgreSQL-only，
  SQLite 使用单独部署示例。
- [ ] 任何实际转发路径变更均在明确授权后通过付费真实上游 smoke；否则对应里程碑仍阻塞。
- [ ] 只有 I-12 同 PR 的预激活 gate 通过后才落最终最小激活改动，重跑通过后才作出可部署声明。

明确非目标：

- NFS/共享文件、多个 Gateway 进程/副本、HA、自动 failover 或跨主机 SQLite 复制；
- 用 SQLite 替代 PostgreSQL 默认部署；
- PostgreSQL 与 SQLite 在线双写、自动双向迁移或零停机切换；
- `sqlx::Any`、插件式第三方数据库、通用 ORM 或公开 repository trait SDK；
- 降低 durability、billing、Codex、MCP、统计或 Console 功能以提前发布。

## 9. 估算与并行方式

总量约 **100–150 工程日**，不确定性主要来自 Codex 外部副作用恢复、两套 API 级幂等与 operator
reconciliation、quota fence/anchor generation 恢复、日志故障注入、统计性能和 SQLx SQLite
transaction 生命周期。该范围不是承诺工期，应预留
约 ±20% 的环境、评审和缺陷修复
波动。

- 单工程师串行：约 **5–8 个月**。
- M2 后由 3 名熟悉代码的工程师分别承担控制面、日志、Codex 三轨：约 **14–21 周**，包含 M10
  收敛和 M11 运维门禁。M3–M5、M6–M7、M8 可部分重叠，但 I-09 必须等待 M5+M8，I-10 必须等待
  I-09+M7；共享 facade/migration、M10 profile 固化、I-12 两阶段 gate 和跨轨缺陷修复仍需串行
  协调，因此不能用 100–150 工程日简单除以三。
- 不通过减少测试、缩短 crash matrix 或提前激活来压缩日历时间。

## 10. 风险登记

| 风险 | 影响 | 缓解与阻断门禁 |
| --- | --- | --- |
| SQLite 单 writer 导致控制面或日志饥饿 | backlog、Console 超时 | 共享公平 coordinator、有界 batch、M5 配置大小/锁时长阈值、M10 数值 qualification profile；M5/M6/M10/M11 阻断 |
| 生命周期被 main/CLI 重复创建 | 双锁、双 worker、关闭次序破坏 | 唯一 `DatabaseRuntime` owner、构造可见性和架构测试；M2/M10 阻断 |
| 仅依赖规范路径/旁路锁使 hard-link 或 bind/mount alias 绕过单进程排除，或锁后路径被替换 | 两个 runtime 同时迁移/写同一 DB，导致损坏或关闭/checkpoint 竞态 | 冻结平台/文件系统/内核锁支持矩阵；no-follow 原子 create/open；DB/WAL/SHM/旁路 lock 的 owner/mode/link-count 校验；记录稳定 file identity 并持有绑定 descriptor/identity 的 OS 排他锁；pool/migration 前复验锁定 identity/path，父目录 0700；同路径/编码/hard-link/bind-mount 两进程与低层 fallback tests；能力不可验证 fail closed；M2/M11 阻断 |
| SQLite 重开/回收连接继承默认 pragma | `synchronous` 降级、外键失效或 busy 行为漂移 | lifecycle/control/log 每个 physical connect/reconnect/recycle 都 set + read-back FULL/foreign_keys/busy policy 并验证 WAL，不匹配连接销毁；M2/M11 阻断 |
| checkpoint/重放顺序错误 | 丢日志或重复扣费 | 明确两条“不得提前”不变量、kill/restart crash matrix；M6 阻断 |
| Decimal 被 SQLite 隐式转浮点 | 金额漂移 | TEXT adapter、Rust Decimal fold、禁止 REAL/f64/SUM(TEXT)、对抗 fixture；M6/M7 阻断 |
| Codex rotating refresh 在 provider 轮换后、持久化前崩溃 | 新 token 不可恢复，旧 token 重试扩大损失 | durable dispatched 状态，abandon/expiry 转 unknown/reauth 且不自动重试旧 token；M8/M9 阻断 |
| Console retry 被误当成新的 Codex reset 意图 | 重复消费 credit，或相同 key 错绑另一 actor/凭证 | required 且全局唯一的 `Idempotency-Key`、actor/credential/action intent fingerprint、单一 durable `redeem_request_id`、同 intent replay/不同 intent conflict、terminal response-loss failpoint；M8/M9 阻断 |
| reset recovery lease 接管与迟到 completion 竞争 | terminal 被覆盖，或重复 reset event/audit | stable `redeem_request_id`、single-active claim/generation/lease、expected-attempt terminal CAS、immutable result、operation ID 唯一事实和 success/`already_redeemed` race failpoint；M8/M9 阻断 |
| pending reset 与 quota response 乱序 | 手动换窗被不可逆误记为 `openai_official`，或历史缺口 | quota attempt dispatch 前注册、fence generation 与 durable boundaries、跨界 observation 一律 ambiguous、fresh post-terminal anchor、原子 cutoff seal 与 attempt-version drain；`checked_at` guard 只在 fence 外；双后端跨实例 crash suite；M8/M9 阻断 |
| quota attempt pre-dispatch/expiry/close 后 response 到达 | 伪造 observation，或已完成历史被重开/覆盖 | completion 只从 dispatched CAS，registered 只能取消/expire，expired immutable；close 前完成/expire 全部 pre-boundary attempt，保留 terminal attempt/fence generation，late response generation rejection；M8/M9 阻断 |
| quota registration 与 fence close 竞争 | attempt 既未 drain 也未被 fence 外 guard 接管 | `active -> draining` CAS 原子捕获 max attempt version，seal 后注册等待/重试，registration-vs-close failpoint；M8/M9 阻断 |
| post-terminal anchor timeout/crash 或 provider 长期不可用 | fence 永久卡住，或错误超时逻辑静默放开后误分类 | fence-owned 单调 anchor generation、每代 single-active claim/lease、失败代 immutable、recovery CAS replacement、旧代 response rejection、visible/alerted active fence、provider 恢复后 eventual-closure suite；M8/I-10/M10/M11 阻断 |
| 两个管理员对 unknown reset 作冲突 reconciliation、resolution response 丢失或运维绕过 API 手改 DB | 重复/错误 manual event、audit 或 anchor side effect | admin-only OpenAPI endpoint、ETag/If-Match CAS、required resolution idempotency key、immutable result、同 key replay、event operation-ID 唯一、concurrent resolver/response-loss/audit-redaction tests；runbook 明确 DB 手改不受支持；M8/I-10 阻断 |
| 把 rotating refresh unknown 误送入 reset resolver | 未知 token 被错误恢复并继续自动使用 | resolver 仅接受 reset-credit operation 类型/状态，refresh unknown 固定 reauthorization 且有 typed rejection 与 runbook；M8/I-10 阻断 |
| reset unresolved 或 quota fence draining 时 credential 被删除、重导入或换 pool | side effect 失去凭证归属、fence/audit 被孤立或静默丢弃 | pending/dispatched/recovering/unknown 及 terminal active/draining fence 的 destructive-mutation typed conflict、batch 原子拒绝、unknown 显式 operator reconciliation、tombstone 后历史保留；M8/I-09/I-10 阻断 |
| reset 长期 pending 导致 quota fence backlog | quota 可见性陈旧、队列持续增长 | pending/recovering/unknown 可观察与告警、有界 claim/lease 恢复、attempt expiry、fresh-anchor/drain backlog 指标和容量阈值；不得猜测失败后放开 fence；M8/M9/M10/M11 阻断 |
| PostgreSQL trigger/约束未显式移植 | projection 或授权不一致 | 共享 mutation contract、SQLite 同事务显式投影、schema/状态审计；M5/M9 阻断 |
| 统计 SQL 方言与时区差异 | API 数值不一致 | canonical fixture、Rust 聚合、DST/边界/golden tests；M7 阻断 |
| 文件系统/权限/第二进程误用 | 损坏或 secret 泄露 | 支持平台正向 local-FS/identity/lock allowlist、unknown 或能力不可验证 fail closed、owner/mode/symlink/link-count 校验、文件身份排他锁、官方容器卷；保证仅限冻结组合；M2/M11 阻断 |
| 双 pool 被误认为双 writer 容量 | 锁争用和错误调参 | 指标区分 logical pool 与全局 writer、生产容量测试；M2/M6 阻断 |
| facade 泄漏 concrete backend | 后续分支扩散、测试组合爆炸 | 静态 grep/架构测试、PR review checklist；M1/M10 阻断 |
| migration 首发后不可恢复 | 启动失败或数据损坏 | 前向 migration fixture、故障中断/恢复、数据库+spool coherent restore drill；M11 阻断 |
| 备份凭据、对象存储权限或恢复密钥泄露/丢失 | 数据泄露或无法恢复 | 最小权限、加密、凭据轮换/失效、恢复演练和异机副本；M11 阻断 |
| 未经资格测试即宣传 SQLite | 超容量部署、数据风险 | I-12 单 PR 两阶段 gate、数值 profile、最终 gate 前禁止宣传；M11 阻断 |
| 转发接缝变化只通过 mock | 真实 provider 回归未发现 | 明确授权后的付费 smoke 是硬门禁；不可用则受影响里程碑阻塞 |
| 功能新增造成计划 inventory 漂移 | “完成”但不等价 | 每个 PostgreSQL schema/feature PR 同步本计划和 SQLite contract；持续治理 |
| 估算受跨轨冲突放大 | 延期、集成返工 | M1/M2 先冻结 contract、三轨 ownership、M10 前定期合并验证 |

## 11. 计划治理

1. 上表是唯一里程碑 checklist。状态只可使用 `[ ] 未开始`、`[~] 进行中`、`[x] 完成`、
   `[!] 阻塞`，并在变化时记录 PR 链接、日期和未过门禁。
2. 每个实施 PR 必须在标题或描述链接本文件及对应 `M# / I-##`，列出进入条件、退出条件、实际测试
   和遗留风险。
3. 不采用“再决定下一个 slice”的临时推进方式。下一 PR 必须来自依赖图中已解锁的固定里程碑。
4. 若需要改变范围、依赖、PR 数量、事务不变量、拓扑或激活门禁，先提交并评审本文更新，再开始
   对应代码变更。
5. 一个 PR 可以加强后续门禁，但不能删除、推迟或用后端特例削弱后续门禁。临时 fallback 必须在
   同一 PR 删除，或明确阻断该里程碑完成。
6. 每个里程碑结束时更新状态和实际工程日；I-09 在 M5 与 M8 完成前不得开始，I-10 在 I-09 与
   M7 完成前不得开始，M5/M7/M9 全部完成前不得开始 M10。
7. I-12 保持一个实施 PR：预激活 gate 证据必须先经评审，再落最终最小激活改动并重跑。最终 gate
   前不得发布 SQLite 可部署宣传；持续 qualification/performance 以及任何因转发路径变化触发的
   真实上游 smoke 都必须先获明确授权，未获授权时状态记为 `[!] 阻塞`，不能记作跳过。
8. M8/I-09/I-10 不得以 `checked_at`、HTTP response 到达时间或内存锁替代 durable quota/reset
   ordering facts，也不得为解除 credential mutation fence 删除 unknown operation。任何修改
   provider `already_redeemed` 解释、attempt lease/expiry、anchor generation replacement/
   stale-response/closure 条件、operator reconciliation 的 admin/ETag/idempotency/outcome/event/
   redaction 契约、rotating-refresh reauthorization 边界、原子 drain cutoff、terminal 后 mutation
   fence 或历史保留策略的 PR，必须先更新本专项设计、OpenAPI/generated types、
   migration/contract/failpoint matrix、Console tests 和 runbook，并经 Codex side-effect 审核。
9. 当前行为发生变化时同步更新[数据库与控制面架构](database-architecture.md)及专题当前文档；
   本提案不能被用来证明功能已经实现。

## 12. 相关文档

- [数据库 Repository 契约与 M1 方法台账](database-repository-contracts.md)
- [数据库与控制面架构](database-architecture.md)
- [请求日志耐久化流水线](request-log-durability.md)
- [统计页面设计](statistics.md)
- [Codex OAuth Connector 设计记录](codex-oauth-connector.md)
- [当前架构](architecture.md)
- [MCP 服务架构与扩展边界](mcp-services.md)
- [生产配置与容量调优](../user/production-configuration.md)
- [文档规范](../documentation-standard.md)
