# SQLite schema、编码与约束映射

> 状态：当前。SQLite 使用独立有序历史；本文区分当前 schema 与不可改写的早期 baseline。

维护仓储时先选择正确的列适配器，再确认 SQL 写入满足数据库保护。
SQLite 不接受 PG 原始 SQL 的逐字替换，也不依赖 affinity 或应用断言补救错误编码。
当前表结构以完整 migration 和 canonical inventory 为准，不以初始表数为准。

## 目录

- [当前 schema 与历史 baseline](#当前-schema-与历史-baseline)
- [存储类型与连接函数](#存储类型与连接函数)
- [精确金额](#精确金额)
- [数据库保护与写入契约](#数据库保护与写入契约)
- [延迟路由约束](#延迟路由约束)
- [索引、错误与验证](#索引错误与验证)

## 当前 schema 与历史 baseline

install_schema() 在独立 SQLite 历史上运行完整 manifest。
当前 0001–0012 不仅包括最早 baseline/guard，还包含 Codex 外部调用 fencing、
独立凭证、新拓扑、六操作、逻辑渠道授权、请求时归属、原生连接器、
插件生命周期及拼车移除。迁移事务和文件身份见[生命周期](sqlite-lifecycle.md)。

| 当前领域 | 结构与所有权 |
| --- | --- |
| 身份 | 用户/组、会话、邀请、注册码、Key/Policy |
| 上游拓扑 | 凭证、接入、管理组、逻辑渠道、操作能力 |
| 路由 | 计价模型、profile、操作规则、能力 tier/candidate |
| 授权 | Key/Policy 固定逻辑渠道与来源 |
| Codex | OAuth 扩展/flow、quota 窗口/reset、组可见性；不再有 pool/自动投影配置 |
| 财务 | ingress、不可变 fact/credential attribution/receipt、pending |
| 查询与审计 | request_logs、排行榜周期/条目、audit |
| 历史/内部 | 只读身份注册表、owner 身份/history、延迟路由 helper、Codex intent 等 |

### 历史 baseline 的作用

0001_baseline.sql 对齐 PG 0001–0063 的最终结构：
34 个 STRICT 业务表、401 列、56 个 FK、183 个原 CHECK 名称；
PG 当时有 34 个 PK、19 个 UNIQUE、55 个非内部 trigger，
SQLite baseline 有 57 个显式索引。数字只用于历史对照，不是当前 schema 总量。
SQLite 保留业务效果，不要求同样物理对象数，ingress 用
INTEGER PRIMARY KEY AUTOINCREMENT，不模拟 PG sequence。

0002_guards.sql 安装跨表/不可变/派生/延迟约束；
0003_codex_operations.sql 安装 SQLite 私有维护 intent。
baseline 的旧 pool 与 paired projection 后续已迁移退役，不是当前仓储写入协议。
不改 baseline 以假装从未存在旧结构。

tests/fixtures/sqlite-schema-inventory.json 服务冻结历史迁移测试；
canonical-schema-inventory.json 是当前结构对照。
新增 schema 同步两后端、当前清单和测试，不覆盖历史夹具。
不重放 PG 历史数据回填/枚举提交屏障，保留默认用户/管理员组 seed；
系统探测身份由仓储创建，不是静态 seed。
request_logs 物理 billed_at 不属于独立事实 schema。

## 存储类型与连接函数

适配位于 sqlite/{types,decimal,functions}.rs，不改变领域类型。
每个物理连接注册 schema 函数/排序规则；未注册连接写入 fail closed。

| PG 类型/语义 | SQLite 编码与保护 |
| --- | --- |
| UUID | 规范小写连字符 TEXT，SqliteUuid/CHECK 拒绝其他编码 |
| enum | TEXT + CHECK；ag_api_format 使用领域枚举顺序，不按字面排序 |
| boolean / integer | STRICT INTEGER；布尔 0/1，16/32 位及原计数范围/NULL |
| timestamptz / date | 固定 UTC 六位微秒加 Z / YYYY-MM-DD，四位年份；量化匹配 SQLx PG 相对 2000 纪元编码，覆盖纪元前亚微秒及闰秒归一化 |
| varchar / char / text | 显式长度/币种/NUL 检查，不依赖声明长度 |
| bytea / cidr | BLOB/hash 长度及网络前缀校验 |
| JSONB | ag_json_valid 解析，递归拒绝 NUL/孤立 surrogate；保留 object/array/NULL/空对象要求 |
| UUID[] / text[] / enum[] | JSON 数组，检查元素类型、枚举、NULL 成员、非空与子集，不用分隔字符串 |
| NUMERIC | 规范十进制 TEXT，精确比较/精度/范围/符号检查，禁止 REAL/NUMERIC affinity |
| lower / 正则 / ANY/ALL / array_position | 专属纯函数，保留字段规则和 NULL/空数组语义 |

ag_lower 使用逐 Unicode 标量简单小写映射；历史测试与 PG en_US.utf8 的
ASCII、重音、土耳其 İ、希腊词尾等对照，不承诺任意 locale/ICU 排序一致。
JSON 不保证 PG JSONB 展示空白或对象键顺序，读取按类型解码。

## 精确金额

| 适配器 | 对应列 | 使用 |
| --- | --- | --- |
| SqliteAmount | numeric(24,8) | 余额、费用、额度、排行榜、邀请码 |
| SqliteUnitPrice | numeric(24,12) | 有效单价和倍率 |
| SqliteTokenRate | numeric(14,4) | output_tokens_per_second |

构造/绑定/解码检查精度，不在适配器内舍入；解码恢复 PG/SQLx 显示 scale，
零保留 PG scale 0。通用 SqliteDecimal 仅作规范 TEXT 传输，不替代列类型。
写仓储按既有列规则量化后绑定，比较重放也用相同规范。
余额允许负以保留软配额透支；价格快照全有/全无、USD、符号和费用资格由数据库检查。

禁止金额 TEXT 字典序、SQLite SUM、浮点或 NUMERIC/REAL CAST。
账户用 checked Decimal，统计用精确整数中间态后进入既有报告结果边界，
见[计量与结算](sqlite-metering.md)。不能把审计/日志 JSON 当作金额来源。

## 数据库保护与写入契约

| 不变量 | 当前保护 |
| --- | --- |
| 事务时钟 | BEGIN IMMEDIATE 后设置 ag_now；updated_at guard 拒绝非当前事务值 |
| audit / fact / attribution / receipt 不可变 | 禁止 UPDATE/DELETE |
| 查询日志 | 禁 UPDATE，允许 DELETE，不级联移除财务证据 |
| 回执资格 | 校验事实分类、精确金额/币种及 Key/user 归属 |
| pending / ingress ack | pending 无回执不可删除/修改；ack 要求 metered、事实及日志投影 |
| 模型身份与墓碑 | 已路由计价身份不可变；墓碑不可恢复/硬删/新引用 |
| 路由提交完整性 | 延迟 FK assertion，允许事务中间形态，最终必须完整 |
| 上游拓扑 | 凭证/接入连接器与范围、绑定/能力身份、生命周期及 revision 保护 |
| Codex intent | 未决维护的受保护更新 fence；迁移后重新安装，不靠重启清除 |
| 操作/格式 | 明确 NOT NULL 与匹配规则，旧 journal 在写入前规范化 |

SQLite BEFORE trigger 不能给 NEW.column 赋值。
不通过“重发整行再 RAISE(IGNORE)”模拟 PG：那会破坏 ON CONFLICT、行数和幂等。
后端私有写入必须显式完成：

1. 为拓扑绑定、操作、凭证归属及其他必需字段绑定明确规范值，不靠旧投影触发器补空值。
2. 请求 operation 使用当前枚举，旧事件走 legacy_default/journal 归一化，不在 INSERT 后绕过 NOT NULL 修补。
3. 带 updated_at 的更新显式 SET updated_at=ag_now()，派生更新同样遵守，
   不混用系统时间或 CURRENT_TIMESTAMP。
4. Codex 导入只写凭证，不创建旧 pool/paired channel；能力开关与授权均显式配置。
5. 数据库 guard 继续拒绝不合法最终状态，不能只在应用中检查。

历史 0001–0004 的仓储夹具仍可能使用旧 pool/投影规则，不能据此编写当前 SQL。

## 延迟路由约束

_gateway_routing_shape 计算规则/tier 完整性。
每条规则对应 _gateway_routing_assertions，其 valid 通过
DEFERRABLE INITIALLY DEFERRED FK 引用唯一不可改 _gateway_true(value=1)。
规则/tier/candidate 变化立即刷新 assertion，提交时才要求全部有效。

helper 行不能伪造或提前删除；合法父规则删除/ID 更新可以级联。
tier/candidate 不得跨 rule 移动。
测试要覆盖删旧建新、空 enabled rule/空 tier 提交失败、
helper 绕过、级联及同 tier 精确候选去重，不用通用 deferred-FK 测试替代业务 shape。

## 索引、错误与验证

索引保留活动自然名/source-model 部分唯一、操作/绑定唯一、精确候选唯一和 UUID 幂等。
历史 Codex NULLS NOT DISTINCT 由表达式唯一索引模拟：
CHECK 已禁止合法空 identity，空串才能安全代表 NULL，而不是随意合并空值。

StorageError 按实际 SQLite 码分类：BUSY/LOCKED 为 Conflict，
约束/类型/长度/schema 函数拒绝为 InvalidInput；
有限路由依赖名称映射 RoutingDependency，其他仍 Internal。
不套用 SQLSTATE，也不把任意 trigger 文本暴露给客户端。

tests/contracts/sqlite_schema.rs 验证完整历史 baseline 安装/重开、后续失败整批回滚、
合法 SQL 与直接负例、upsert、金融/墓碑/路由保护。
sqlite_parity.rs 对照真实 PG schema、seed、枚举、金额显示、时间及历史派生 UUID。
当前拓扑/财务/维护契约运行完整启动迁移，另保留每次存量升级/失败回滚。
进程、别名、取消和崩溃检查见[生命周期](sqlite-lifecycle.md)；
部署与恢复见[用户指南](../user/sqlite.md)。
