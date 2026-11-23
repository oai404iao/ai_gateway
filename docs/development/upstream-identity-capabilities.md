# 上游身份、渠道能力与路由目标

> 状态：当前。

上游凭证回答“以谁的身份认证”，接入回答“连接到哪里”，逻辑渠道绑定二者，
能力声明具体操作，路由候选决定向哪个能力发送什么 wire model。
客户端 API Key 是独立的入站身份，不能与上游凭证混为一体。

## 目录

- [实体与所有权](#实体与所有权)
- [凭证与目标范围](#凭证与目标范围)
- [六种操作与显式路由](#六种操作与显式路由)
- [逻辑渠道授权与能力删除](#逻辑渠道授权与能力删除)
- [统一凭证与逻辑渠道](#统一凭证与逻辑渠道)
- [运行时与连接失效](#运行时与连接失效)
- [验证](#验证)

## 实体与所有权

```text
上游凭证 ───┐
            ├─ 逻辑渠道 ── 渠道能力 ── 能力模型目录
上游接入 ───┘      │
                   └─ 管理组

客户端计价模型 ── profile ── 操作规则 ── priority 层
                                          └─ (能力, wire model, weight)
```

| 实体 | 唯一配置来源 |
| --- | --- |
| 上游凭证 | 稳定 UUID、连接器 ID、认证类型、静态认证材料/精确目标范围及生命周期 |
| Codex 扩展 | OAuth Token、账号、维护代理、账号模型目录、generation 与额度窗口 |
| 上游接入 | 连接器 ID、Base URL、代理与超时 |
| 逻辑渠道 | 接入/nullable 凭证绑定、管理组、总开关与 binding revision |
| 渠道能力 | 操作、模型目录、独立健康/开关、变换、压缩、探测、倍率及状态统计开关 |
| 管理组 | 组织关系与管理开关，不拥有格式、连接器或认证 |
| 路由候选 | 显式 capability、wire model、weight；由 tier 保存 priority 和 strategy |
| Key / Policy 目标 | 显式选择的管理组 UUID 和直接选择的逻辑渠道 UUID，不是路由候选 |

创建或导入凭证、加载插件都不隐式创建渠道、启用能力或添加候选，也不授予 Key 访问权。
管理组不是多对多标签，也不根据成员变化动态扩展路由。
没有第二层隐式凭证选路、自动协议降级、跨格式重试或万能中间协议。

## 凭证与目标范围

### 静态凭证

Bearer 或指定 Header 的 secret 只在凭证保存，不在渠道复制。
相同 secret 不代表同一业务身份，不按 secret 自动去重。
认证类型不可原地修改；改类型须新建凭证并显式改绑。

目标范围使用规范化后的完整 Base URL **精确匹配**，不是域名、子域或路径前缀：

- 仅 HTTP/HTTPS；拒绝 userinfo、query、fragment、首尾空白、控制字符和反斜杠。
- 使用同一个 Rust URL 规范化器处理保存、绑定、发现和编译。
- scheme/host 大小写及默认端口等价，尾部斜线按端点拼接语义移除。
- 不合并路径大小写、内部重复斜线、非末尾路径段或保留的百分号转义。
- 不用 SQL lower 或前缀比较代替规范化。

扩大范围是独立的管理员 ETag 修改并审计。
收窄范围必须验证所有未删除渠道，包括 disabled、auto-disabled、无候选或停用组中的绑定。
“临时不可用”不是解绑；共享接入的目标修改也必须验证其全部引用凭证。
检查与写入处于同一事务，不能让并发新增引用绕过保护。

### 管理与秘密

列表不返回 secret；详情按产品策略允许管理员查看静态 secret。
创建必须有合法认证材料及非空范围；更新省略 secret 表示保留，显式新值表示轮换，
不能静默清空。禁用阻止所有引用渠道的新请求，不删除路由；恢复不打开原来关闭的能力。

渠道写入显式 nullable credential_id，NULL 就是无认证，不造占位凭证。
严格反序列化拒绝旧内联认证字段；模型发现使用相同的凭证、范围和启用检查，
不提供任意密钥注入旁路。绑定还必须满足接入与凭证连接器 ID 一致。
创建、轮换、删除和改绑只记录脱敏审计，错误与 Debug 不输出材料。

普通凭证删除要求没有未删除渠道引用；清除 secret 并保留墓碑。
删除渠道不顺带删除共享凭证。Codex 修改、导入、导出和删除只走专属流程，
通用凭证接口不能改 Token 或伪装 OAuth 身份。

## 六种操作与显式路由

| 操作 | 传输与约束 |
| --- | --- |
| `chat_completion` | Chat Completions HTTP JSON/SSE |
| `responses` | Responses HTTP JSON/SSE |
| `responses-ws` | Responses WebSocket，独立选路、健康和日志操作 |
| `web_search` | 独立非流式搜索，仍使用候选 wire model 及目录门槛 |
| `images_generation` | 非流式 JSON 图片生成 |
| `images_edit` | 非流式 multipart 图片编辑 |

不持久化可配置 transports，不以独立布尔值生成未实现的操作/传输组合。
连接器声明实现上限，管理员只能收窄。已注册原生插件也不能扩大六操作集合。
Codex 对 HTTP 非流式 Responses 的拒绝仍有效，操作存在不代表连接器支持每种请求形态。

计价模型是客户端不可变身份；操作规则按 profile/operation 唯一。
启用规则必须有非空 tier，tier 的候选显式保存能力和上游别名。
同层仅拒绝完全相同的 capability/model；跨层重复允许。
相同模型名不证明操作兼容；搜索不能借“无需模型”改用空串/NULL。
图片生成和编辑分别建规则，HTTP 与 WS 也分别建规则；不混池、不翻倍权重。

定时探测目前仅支持 general 的 Chat Completions/Responses HTTP JSON。
wire test_model 和 test_pricing_model_id 成对设置，不能按 wire 字符串猜价格身份。
WS 不继承 HTTP 压缩和探测；能力表出现不代表 Codex、搜索或 Images 探测已实现。

## 逻辑渠道授权与能力删除

Key 保存所选组 UUID（`allowed_group_ids`）和直接选择的逻辑渠道 UUID（`allowed_channel_ids`）。
每次编译快照时，有效渠道是**所选组的当前成员与有直接授权记录的渠道的并集**。Key 和 Policy 的直接授权分别存于 `api_key_channel_grants` 和 `api_key_policy_channel_grants`，每个所有者和渠道组合只有一条记录。仅存在于选择数组中的渠道不获权；原样保存也不创建授权。

- 渠道加入已选组即获得组授权；移出即失去该组授权。直接渠道授权不依赖组归属。
  其他组的渠道不会因共用上游凭证而获权。删除或未知的 UUID 不授予访问，同名重建也不继承原 UUID 的选择。
- 选择空组仍是一项组授权；日后加入的渠道按当前成员关系展开，无需重签 Key。
- 授权覆盖渠道的现有及未来能力；能力开关、路由候选、模型目录、健康和准入仍独立检查。

Policy 只约束[自助签发及目标变更](console-auth.md#api-key-自助服务规则)，不参与已发 Key 的快照授权计算。

### 删除与依赖

删除能力使用详情 ETag，在同一控制面事务：

- 撤销其全部候选，清理空 tier，停用无候选规则；
- 保留其他候选的模型别名、权重、priority 和 strategy；
- 推进受影响规则 ETag，审计记录规则 ID 与停用结果；
- 写能力墓碑，保留渠道授权和金融历史。

这也适用于 Codex 能力。Console 明确提示自动解绑/停用，并失效能力与规则缓存。
逻辑渠道必须先退役能力，管理组必须先清空渠道；仍被未删除渠道引用的接入/凭证不能删。
模型墓碑删除另会撤销其候选及探测引用，避免永久占用能力，详见[软删除](soft-deletion.md)。

## 统一凭证与逻辑渠道

`routing_groups → upstream_channels → upstream_credentials` 是所有连接器的共同组织模型。
凭证不属于组/pool；导入、重新授权、启停或改名不创建或修改渠道拓扑。

- credential.connector_kind 是显式、不可变连接器 ID，与认证材料类型独立。
  静态凭证默认 general，也可归属已注册原生插件；codex_oauth 只允许 codex。
- OAuth 公共身份 UUID 保持稳定，Token 不复制到通用身份记录。
  专属表历史 channel_id 名称表示凭证 ID，不是当前逻辑渠道外键。
- OAuth 扩展拥有维护代理和账号目录；维护状态不覆盖渠道接入、能力目录或开关。
  删除先解除全部未删除渠道引用，再走专属 Token 擦除和墓碑事务。
- 额度窗口按凭证 UUID 延续。用户可见性经用户组授权的渠道组所引用凭证限定，并按账号去重，
  不是重新引入 pool 所属。
- SQLite 未决 refresh/quota-reset intent 在关闭/重开时保持恢复边界；重启不会把未知兑换变成可安全重试，见[SQLite Codex](sqlite-codex.md)。
- 原生连接器注册表属于进程运行时实例。启动和控制面提交前检查所有未删除接入、凭证、
  能力，包括停用与未绑定草稿；未注册 ID、未声明操作或缺所需命令均拒绝发布。
  插件安装不自动启用，详见[插件设计](connector-plugins.md)。

请求凭证归属与当前绑定分开保存：在选路快照中捕获 UUID 或明确无认证，随事实同事务持久化。冲突不覆盖；后续改绑不移动已完成或在途请求费用。

## 运行时与连接失效

完整控制面记录一次性编译为不可变快照，数据面不逐请求查库。
非法引用、连接器/认证类型冲突、越界目标或禁止认证 Header 使候选编译失败，
回滚整个 mutation；不能发布认证为空的替代配置。
禁用是合法配置状态，选路排除即可。

请求保持取得的身份视图；轮换不承诺撤回已发出的请求。
Codex Token 继续通过独立凭证快照和刷新互斥维护。

WebSocket 池身份包含凭证 UUID/revision、接入 revision、渠道 binding revision、
能力 revision 及既有 session/Key/候选/网络/最终 Header 维度。
轮换、启停、范围/网络/能力变更推进对应版本，不以名称或 updated_at 精度代替。
A → 空/B → A 不能恢复旧连接，引用同一凭证的不同渠道不能因此混池。

闲置、借出和归还连接均验证有效性，仅清空闲池不够。
失效 continuation 返回 404 previous_response_not_found，不换新连接派发；
无 previous-response 的新请求可以重新连接。
凭证状态、接入故障、能力失败及能力/模型失败按作用域隔离，不复制提供方 quota。

入站/出站白名单、插件隐私、用户权限、Images 不可重试和 WS 连接局部状态均维持，
该拓扑不引入协议转换或绕过准入。

## 验证

- 在同一组用例中验证单凭证轮换、多渠道复用、范围收窄与停用引用保护。WS 测试应分别在借出与归还旧连接时验证绑定 revision；`A → B → A` 也不得复用旧连接。
- 组成员变化只影响有效授权，不能扩展路由候选。分别检查选择组、直接渠道、两者并存以及同名重建的 Key。
- 删除能力应修剪空层、停用空规则、推进相关 ETag。旧请求费用仍按原凭证归属。

实现位于 [`upstream_credentials/`](../../src/persistence/upstream_credentials/) 与 [`upstream_topology/`](../../src/persistence/upstream_topology/)。在两个后端验证变更失败时事务回滚，既有授权、引用和账本不变。

## 相关文档

- [数据库架构](database-architecture.md)、[当前架构](architecture.md)
- [Codex Connector](codex-oauth-connector.md)、[插件设计](connector-plugins.md)
- [运维接口](../user/routing.md)、[上游凭证管理](../user/upstream-credentials.md)
