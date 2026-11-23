# Console API、JWT 与角色授权

> 状态：当前。

Console 使用独立 listener 上的 `/console/v1/*` API，普通用户和管理员通过同一登录机制进入。
`admin` 是用户角色，不是路径命名空间或静态 Bearer 密钥。公共 `/v1/*` 仍使用客户端 API Key，
不接收 Console JWT，也不逐请求查询认证数据库。

## 本页内容

- [配置与初始化](#配置与初始化)
- [理解账户创建与密码恢复](#账户创建与密码恢复)
- [跟踪 Token 与会话](#认证和授权)
- [限制本人资源和管理员写入](#角色与资源归属)
- [修改认证数据模型](#数据模型)
- [检查 API Key 自助服务](#api-key-自助服务规则)
- [验证安全边界](#验证)

## 配置与初始化

Console 在单独的监听器上提供服务。TOML 只接受 Ed25519 密钥文件路径，不接受私钥内容或静态管理员 Token；带凭据的浏览器请求须匹配显式配置的 CORS origin。认证请求、普通 Console API 和插件上传分别受大小限制。配置示例与首个管理员的 stdin 初始化见[快速上手](../user/quickstart.md)；SQLite 管理 CLI 须[停机取得 owner 锁](../user/sqlite.md)。

## 账户创建与密码恢复

| 入口 | 当前行为 |
| --- | --- |
| Bootstrap CLI | 初始化首个管理员 |
| 管理员邀请 | 创建一次性、有过期时间的激活 token；外部系统负责发送 |
| 注册邀请码 | 匿名用户使用管理员维护的可复用码注册，事务检查启用、过期和剩余次数 |
| 紧急管理员恢复 | CLI 为已有 active 管理员重置密码，必须从 stdin 读取 |
| 管理员临时密码 | 重新验证管理员当前密码后，为其他 active、已设置密码用户签发 24 小时临时密码 |

注册成功即创建 active 用户、分配邀请码当前用户组与初始余额，并签发 Console Session；
不经过邮箱确认。注册邀请码明文只在创建时返回一次，数据库只保存哈希。
一次性邀请同样只保存 token 哈希；重新邀请适用的未设密码用户时撤销旧邀请并恢复 `invited` 状态。

临时密码签发替换原哈希、递增 `auth_version`、撤销全部 Console Session 并要求改密，
但不撤销客户端 API Key。临时密码登录只能得到 `purpose = password_change` 的受限 Session。
完成改密前，middleware 只允许刷新、退出和完成密码重置；提交的新密码不能等于临时密码。
完成事务清除临时状态、撤销受限 Session，再签发普通 Session。

## 认证和授权

```text
login / invitation activation / registration
  -> Argon2id password or one-time credential validation
  -> short-lived EdDSA access JWT + rotating refresh Cookie
  -> JWT verification
  -> current database user + session + auth_version validation
  -> route authorization
  -> owner-scoped query or transaction-level admin recheck
```

Access JWT 包含 `sub`、`sid`、`role`、`auth_version`、`iss`、`aud`、`iat`、`exp`、`jti`，
经 `Authorization: Bearer` 发送。JWT 验签成功不等于授权成功：middleware 仍检查用户状态、
当前角色、Session 撤销与认证版本，使禁用、改密、登出和角色降级及时生效。

Refresh token 是高熵 opaque 值，只通过 `HttpOnly; Secure; SameSite=Lax` Cookie 传递；
每次刷新都轮换，重放旧 token 会撤销该 Session。数据库永不保存明文 refresh token。
前端 access token 只驻留内存，不写入浏览器存储、URL 或持久 Query Cache。
登录与密码验证必须限流，密码和 token 不进入日志、audit 或错误摘要。

路径、请求字段、Cookie 和错误码见 [Console OpenAPI](../openapi/console-v1.yaml)。例如完成临时密码重置要求 `password_change` 会话，不能仅按路径前缀把所有 `/auth/*` 当作匿名端点。

## 角色与资源归属

| 资源 | user | admin |
| --- | --- | --- |
| 本人资料、安全设置、Session、Key、日志、用量与费用 | 仅本人 | 仅本人入口同样按主体限定 |
| 本人可见 Codex quota | 按当前用户组查询，只读、去重和最小投影 | 可另用管理员入口 |
| 用户、用户组、注册邀请码、Key Policy | 否 | 是 |
| 模型、操作路由、接入、凭证、渠道、代理、模板与插件 | 否 | 是 |
| 全局请求日志、审计、重载和系统设置 | 否 | 是 |

本人资源使用 `/console/v1/me/*`。repository SQL 必须以 JWT 主体为过滤条件，不能信任 path
或 body 中的 `user_id`。Session 撤销也只能作用于本人；批量“退出其他会话”保留当前 Session。

控制面 mutation 在事务内重新确认 actor 仍为 active admin，不能只依赖页面守卫或先前 JWT。
PostgreSQL 控制面使用 serializable 事务；SQLite 通过同一操作契约进入 owner/writer 边界。
管理员插件 mutation 另需短时一次性重新授权，见[插件操作安全](connector-plugins.md)。

## 数据模型

| 表 | 维护约束 |
| --- | --- |
| `users` | email 大小写无关唯一，role 为 user/admin；余额统一 USD；密码保存 Argon2id 哈希 |
| `user_sessions` | refresh 哈希、状态、过期和轮换时间；最长 512 字符 User-Agent 用于设备区分，可为空 |
| `user_invitations` | 一次性邀请哈希、过期与撤销事实 |
| `api_key_policies` | 用户可选择的组/逻辑渠道上界，不保存 RPM、并发、额度或权限 |
| `user_groups` | 每人唯一所属组及默认 Policy；受保护的默认用户组/管理员组不可随意删除 |

`users.auth_version` 在认证状态改变时递增，`password_changed_at` 保留安全时间信息。
用户级 `default_api_key_policy_id` 可覆盖组默认值；为空时动态继承所属组。
遗留用户可暂时缺少 email/password，但不能因此登录。邀请用户为 `invited`，激活后为 `active`。

删除用户是不可恢复匿名化：撤销 Session、邀请和 Key，保留主键及 `deleted_at` / `deleted_by`
以维护日志和审计引用。删除自定义用户组时按角色迁移成员，细节见[墓碑与解绑](soft-deletion.md)。批量用户修改核验所有版本，任一失败回滚整批。

## API Key 自助服务规则

有效 Policy 按“用户覆盖，否则组默认”解析。普通用户从上界中选择
`allowed_group_ids` / `allowed_channel_ids`，可设置自身 Key 的 RPM、并发与额度，但不能传入
`user_id`、API 格式或权限；自助权限固定为 `proxy`、`models.read`。

自助签发或修改授权目标时，只有 Policy 显式选择了某个组，用户才能选择整组。
用户也可直接选择 Policy 指定的渠道或其已选组的当前成员；即使 Policy 逐一指定了组内所有渠道，也不等于授予整组。
Policy 更新不追溯改变已发 Key；仅修改 Key 名称、额度等元数据不重新签发授权。
已发 Key 的[有效渠道](upstream-identity-capabilities.md#逻辑渠道授权与能力删除)由 Key 自身的选择和当前拓扑决定。

## 验证

认证、ownership、refresh 重放、临时密码与规范漂移由 `tests/console_spec_integration.rs` 等
集成测试约束；前端用组件/浏览器测试验证刷新和受限会话 UX。

修改这些边界时检查：

- 其他用户的 Key、日志和 Session 均不可读写或撤销。
- body 不能扩大权限，JWT 旧角色不能绕过事务内管理员复核。
- 禁用、密码变更、登出、refresh 重放和降权使对应 access token 失效。
- audit 归属真实 actor，不记录密码、明文邀请或 refresh token。
- 各路由 body limit 独立返回安全的 `413`。
- 公共数据面不因 Console 鉴权改为数据库逐请求校验。

## 实现入口

当前路由注册在 `src/http/console.rs`，认证编排在 `src/application/auth.rs`，
用户操作见[运维接口](../user/console.md)，UI 会话约定见[Console UI](console-ui.md)。
