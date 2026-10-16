# 安全运行与账户恢复

> 状态：当前。

网关持有客户端 API Key、上游密钥和 OAuth Token，并能代表调用者向上游发起付费请求。安全边界依赖操作系统权限、数据库和备份保护、可信插件以及正确的网络入口；不要把私有监听地址、日志脱敏或软额度当作完整隔离。

## 选择暴露范围

| 入口 | 认证与部署要求 |
| --- | --- |
| 公共 `/v1/*` | Gateway API Key；按实际消费者设置防火墙、TLS 和反向代理 |
| `/health` | 无认证，返回 `204`；不证明业务、数据库或结算就绪 |
| Console `/console/v1/*` | 登录用户的 JWT；独立监听器，浏览器必须通过 HTTPS |
| 嵌入 UI | 只由 Console listener 提供，不能放到数据面端口 |
| PostgreSQL | 默认仅宿主 loopback 或 Compose 内部网络，不应直接暴露公网 |

Gateway 不终止 TLS。反向代理需允许流式响应和 WebSocket Upgrade，并设置覆盖模型响应时长的超时；不要无意删除用于会话隔离的允许 Header。`allowed_origins` 只列真实浏览器来源，不使用 `*`；CORS 不能替代鉴权和网络访问控制。

## 保护配置与凭证

- 首次创建秘密前设置 `umask 077`；限制配置目录、JWT 私钥、数据库密码和导出文件的读写权限。
- 不提交 `config/config.toml`、JWT 私钥、数据库密码、真实 Key、`.env.real-upstream` 或 OAuth Bundle。
- 普通用户和管理员可以按产品权限查看已保存的客户端 API Key；管理员还可从通用上游凭证详情读取静态密钥。不要假设这些值只有创建时可见。
- 常规 Codex 列表和详情不返回已保存 Token；管理员显式导出会获得原始 Token 和代理认证信息。按未加密密钥备份处理导出包，不放入工单、截图或日志。
- 数据库及其备份包含秘密；当前未额外提供上游秘密的列级静态加密。文件权限、磁盘加密和受保护备份由部署方落实。
- 上游凭证只能用于明确允许的完整 Base URL；范围匹配不是域名或路径前缀白名单。改地址前先按[凭证指南](upstream-credentials.md)审核范围。

不要在共享终端启用 `set -x` 后执行含秘密的命令，也不要使用 `curl -v` 收集带认证的完整请求。错误详情可能包含上游返回的文本或结构化对象，分享前仍须检查。

## 理解 Console 会话

Access JWT 短期有效，浏览器仅保存在内存，不写 `localStorage`。refresh token 位于 `HttpOnly; Secure; SameSite=Lax` Cookie，服务端只保存哈希；刷新会轮换，重放旧 token 会撤销该 Session。

每次 Console 请求还核对用户、Session 和 `auth_version`，禁用用户、改密、登出或角色变更会使旧 token 失效。设备页可以撤销当前、指定或其他会话；`last_seen_at` 是最近一次 refresh 轮换时间，不是每次 HTTP 访问时间。账户恢复流程见[Console](console.md#协助用户恢复密码)。

## 安装可信插件

原生插件与 Gateway 进程拥有相同权限；安装权限等价于允许执行该权限下的代码。只安装已审查、架构/libc 匹配的制品，先检查来源和摘要，再明确启用。网关不会自动下载 Codex，也没有内置回退。

插件目录由运行用户管理且需单进程独占。容器中的可写插件目录是有意的原生代码管理边界，不是任意上传隔离区；不要原地替换已安装 `.so`。版本切换、保留额度和恢复限制见[插件管理](connector-plugins.md)。

## 保护财务与恢复证据

额度不预留资金，余额允许为负；RPM、并发、健康和 Session affinity 均为进程本地状态，不是多实例共享限额。需要严格支出或隔离保证时，不能仅依赖这些软检查。

request-log spool 是持久财务入口，不能当缓存清理；Images edit spool 只是短生命周期 scratch，不包含可恢复 HTTP 会话。只有 intent、没有完整终态的请求需保留待核对，不能记零、删除 intent 或凭重启宣称退款。Codex 未知 quota-reset 结果同样不能自动重试或清除保护状态。

备份应覆盖同一恢复点的数据库、完整 request spool、配置、JWT 密钥与插件目录，并保留匹配二进制。秘密备份应加密、受限访问，并有独立于源主机的副本；归档成功或 checksum 正确不等于已经验证可恢复。SQLite 使用[离线成对工具](sqlite.md#停机成对备份)，升级恢复见[升级指南](upgrades.md)。

## 紧急重置管理员密码

若 active admin 遗失密码，可在有权读取配置和访问数据库的主机执行 stdin-only CLI；这是主机恢复权限，不是匿名 Console 接口。SQLite 必须先停止服务，管理员 CLI 不能与其运行进程同时持有数据库。

```bash
read -r -s -p '新管理员密码（至少 12 字节）: ' new_password
printf '\n'
printf '%s' "$new_password" | ./target/release/ai-gateway reset-admin-password \
  --config /absolute/path/config.toml \
  --email admin@example.com --password-stdin
unset new_password
```

替换二进制和配置路径为实际位置；源码开发可使用 `./target/debug/ai-gateway`。命令只重置匹配邮箱的 active admin，以 Argon2 哈希保存密码，撤销全部 Console Session 并清除临时改密标记；不会修改数据面 Key、余额或路由。密码至少 12 字节，不输出密码或哈希。

其他 active 用户忘记密码时，管理员可签发 24 小时有效的临时密码；用户必须完成受限改密流程后才能访问普通 Console 资源。不要用临时密码流程替代邀请激活，也不要通过手工改库绕过用户状态检查。
