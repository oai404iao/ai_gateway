# 升级与回滚

> 状态：当前。用于已有部署；全新安装从[快速开始](quickstart.md)或[生产部署](production-deployment.md)进入。

升级会自动执行有序数据库 migration，可能永久移除旧 schema 或改变旧版本能理解的配置。先确认跨越的版本边界、停写和完整恢复点，再替换二进制；不要把切回旧镜像当作数据库回滚。

## 执行升级

1. 阅读目标版本的 [Changelog](../../CHANGELOG.md)，列出跨越的数据库、配置和插件协议边界。
2. 阻止新请求、排空在途工作并停止所有旧实例及 worker，确认自动重启不会恢复旧写入者。
3. 在同一停写点保留数据库、完整 request-log spool、配置、JWT 密钥、插件目录和匹配旧制品。SQLite 必须使用[离线成对工具](sqlite.md#停机成对备份)。PostgreSQL 使用受支持的逻辑/物理备份，不能复制正在写入的数据目录代替一致备份。
4. 在隔离副本验证迁移时长、磁盘/WAL 余量与恢复流程。若跨越 journal 不兼容边界，先排空旧 spool/ingress；兼容版本也必须保留未知 intent，不能为迁移清除证据。
5. 更新目标镜像或二进制、匹配的 TOML 和插件，启动一次迁移，再整体切换实例。
6. 核对健康、Console、授权、日志/结算 backlog 及历史金额；代表性上游请求须单独授权且可能收费。验收前保留旧恢复点。

## 回滚前检查

先停止新写入者并保存切换后的新增数据。只有 schema、配置和插件均兼容时才可只回退应用；否则需要恢复完整配套备份和匹配旧版本，并核对新产生记录的处理方式。不能用旧快照直接覆盖新账本，或同时运行两个恢复副本。

## 上游拓扑硬切换升级

PostgreSQL migration `0065` / SQLite migration `0005` 将旧渠道组、渠道、协议规则和
Codex 双投影一次性转存为管理组、接入、逻辑渠道、操作能力与显式候选，随后删除旧配置表。
启动迁移在同一事务中完成转存、历史外键迁移和完整快照编译；任一步失败整批回滚。
修复后可重试，重复启动不会重复转存。旧日志、金融事实、结算收据和 spool 不改写。

随后 `0066` / SQLite `0006` 拆分六种操作并删除可配置 transports；
`0067` / SQLite `0007` 将能力授权按所属逻辑渠道去重迁移为渠道授权。
已授权渠道的所有能力共享授权，但组中新加入的渠道不会自动授权。
`0068` / SQLite `0008` 删除凭证的组/pool 归属；
`0069` / SQLite `0009` 增加不可变逐请求凭证归属，避免渠道改绑转移历史或在途费用。
原凭证、渠道、能力、额度窗口和普通计量历史均保留；旧未完成的组绑定 OAuth 流失效。
完整快照在整批迁移结束后验证，失败仍回滚整批。

升级必须停止所有旧 Gateway 写入，先备份数据库和 spool；SQLite 使用成对离线备份。
不能新旧版本滚动混跑，也不能把已升级数据库直接交给旧二进制。需要回滚时恢复完整备份及
匹配版本。升级预检拒绝无法表示的停用草稿、非法凭证范围或路由；错误只报告 ID/类别，
请先在旧版本修复或删除这些配置，不能跳过检查或手工清空迁移状态。
更早版本仍须依次满足历史 migration 的别名、价格和路由约束。
迁移边界见[上游身份与能力设计](../development/upstream-identity-capabilities.md)。

## Codex 拼车移除升级

Codex 拼车的固定席位、金额窗口、准入账本及 Console 页面/API 已移除；
普通 Codex OAuth、配额查看、凭证复用与普通计费不受影响。

1. 停止所有旧 Gateway，备份同一停写点的数据库和完整 spool；SQLite 使用
   [成对离线备份](sqlite.md)。保留匹配的旧二进制与配置，禁止新旧版本混跑。
2. 从 TOML 删除整个 `[codex_sharing]` 段，升级二进制并执行启动迁移。
   PostgreSQL/SQLite 的新增有序迁移先停用原 `sharing_only`、已有绑定及同账号别名保护
   覆盖的渠道，再删除专属表、列和约束；不改写旧 migration，
   不删除普通请求日志、计量事实、结算收据、账户余额或 Codex 配额历史。
3. 在恢复流量前逐一审核受影响渠道、模型路由、API Key 和 Policy 的普通授权。
   旧席位不再授予权限，也没有金额窗口保护；确认符合普通使用策略后才手动重新启用渠道。
   自助创建 Key 或修改授权目标必须有有效 Policy，不能再仅凭原席位授权。
4. 原 `request_logging.spool_directory/codex-sharing/` WAL/检查点不会自动删除，
   新版本不再读取或恢复它们。按备份保留策略人工归档或清理，勿与仍使用的请求日志 spool
   混淆。回滚必须恢复完整配套备份和旧版本，不能直接用旧二进制写入新 schema。

## 检查更早版本的迁移边界

只在跨越对应版本时应用下面的兼容性检查；它们不是新部署需要重复执行的手工迁移步骤。

### 移除 MCP

移除 MCP 的版本包含 `0053_remove_mcp.sql`：删除 MCP 实例表、类型和系统设置，将历史
MCP 日志归入 `client`，保留用量与费用，并兼容旧 spool/ingress 日志回放。升级前备份并停止
所有旧实例；不要混跑新旧二进制，也不要仅回滚应用而继续使用已迁移数据库。移除旧配置中的
`[mcp]` 段及构建命令中的 `mcp-server` feature。Images 与独立搜索 HTTP 接口不变。

### PostgreSQL 事务边界

Gateway 会在数据库 migration advisory lock 下，将连续待执行 migration 放在同一个 PostgreSQL
事务中；任一版本失败都会回滚同批已经执行的其他版本。历史 `0034`、`0046` 新增 PostgreSQL
枚举值，因数据库要求先提交再引用而构成事务屏障；`0053–0063` 不含屏障，会整体提交或回滚。
旧 Gateway 此前已经提交的 migration 不属于当前事务，仍需通过升级前备份恢复，不能靠切换旧
二进制撤销。

### 历史费用、显式候选和独立计量

Migration `0061_zero_failed_and_cancelled_costs.sql` 会把历史失败/取消请求改为零费用，并退回
此前已经结算的用户余额与 API Key 已用额度。升级前应保留余额、Key 额度和请求日志的一致备份。

Migration `0062_flat_model_route_candidates.sql` 会一次性展开旧 Channel Group 路由目标，并删除
旧 group/channel 路由表。所有旧 Gateway 必须在应用 migration 前排空并停止；迁移后不能回切旧
二进制。旧 `all` 目标只快照迁移时的当前成员，之后组成员变化不再自动改变模型规则。

Migration `0063_independent_metering_facts.sql` 把历史费用与已结算状态回填到独立事实/回执，
不再次扣款，并删除日志物理 `billed_at`。必须停止全部旧 worker 后整体切换，不能滚动混跑或
仅回滚二进制。该 migration 本身不改变 journal 格式，支持版本的 spool/ingress 可以保留；
未知 intent 必须保留待核对。发布前需用真实规模的隔离副本验证回填时长与磁盘/WAL 余量。
完整备份、恢复与验收要求见[独立计量切换](../development/independent-metering.md)；
手动隔离验证入口见[成对备份恢复演练](../development/persistence-rehearsal.md)。
小数据开发库的通过结果不能代替不同数据量/硬件下的生产停机容量验收。

### 更早的字段与凭证边界

Migration `0017_remove_legacy_compatibility.sql` 会永久删除
`api_keys.tokens_per_minute` 与 `channels.health_check` 的值；升级前备份必须可用。

Migration `0036_codex_images_projection.sql` 在当时的旧拓扑中新增 Codex
`open_ai_images` groups；尚未包含该迁移支持的旧 Gateway 无法编译它们。多实例部署必须先排空并停止所有未包含该 migration 对应代码的旧实例，
再由新版本执行 migration 并整体切换；migration 应用后不得把流量或 Console 请求切回旧二进制。

Migration `0040_codex_optional_account_id.sql` 允许个人 Codex 凭证把 workspace account ID 保存为
`NULL`。旧二进制的持久化类型仍把该列当作必填字符串，无法加载新版本写入的 accountless
凭证；多实例部署应停止旧实例后再升级并导入这类凭证，不能让新旧版本共同维护同一 Codex
credential pool。

### 严格配置与 Images edit

Images edit 不增加数据库 migration，新二进制可以用未包含 `image_edit_*` 的旧 TOML 启动并采用
默认值。但旧二进制的 TOML schema 使用 `deny_unknown_fields`，不能读取新增的
`request_limits.image_edit_*` 设置；滚动升级或应用回滚时，必须让旧实例继续使用旧配置，或先从
其配置中移除这些字段。所有实例完成升级前，也不要把 `/v1/images/edits` 流量发送到混合版本
负载均衡池，否则旧实例会返回 404。


### UTC 计价时段与插件协议

旧版本会拒绝包含 `time_multipliers` 的运行时快照。若必须回到不支持时段字段的版本，先用新版本清空全部模型时段并发布，再确认目标版本仍兼容数据库。原生插件的协议与设置 schema 也须匹配；[插件升级](connector-plugins.md#从旧部署升级)不能通过仅回滚 Gateway 绕过。

## 保留持久卷与证据

不使用 `down -v` 或删除 `postgres-data`、`gateway-spool`、`gateway-plugins` 完成升级。Images scratch 仅可在 Gateway 正常停止后重建；删除活跃 mount 会使在途 edit 失败。Compose 项目名变化也可能选中空卷，切换前核对真实卷身份。具体容器操作见[部署指南](production-deployment.md)。
