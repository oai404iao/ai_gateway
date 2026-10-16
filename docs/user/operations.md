# 运行与接口说明

> 状态：当前。

ai-gateway 将使用 API Key 的公共数据面与使用 JWT 的 Console 分为两个监听器。按你要完成的任务选择下文入口；完整操作已按主题拆分，本页仍保留原有章节入口，供已有链接使用。

## 启动

第一次从源码运行，按[快速开始](quickstart.md)准备数据库、Console 密钥、管理员、路由和第一个请求。生产容器使用[部署指南](production-deployment.md)，文件数据库使用 [SQLite 指南](sqlite.md)。空控制面可以启动，但不能代理请求。

### 上游拓扑硬切换升级

停写、完整备份与新旧版本边界见[升级指南](upgrades.md#上游拓扑硬切换升级)。不能让旧二进制继续写入已迁移 schema。

### Codex 拼车移除升级

受影响渠道会保持停用，旧席位不再授予访问权限。恢复流量前按[拼车移除检查表](upgrades.md#codex-拼车移除升级)逐项审核普通路由与授权，不要删除残留财务证据。

### 紧急重置管理员密码

使用 stdin-only CLI 恢复 active admin，详见[安全与账户恢复](security.md#紧急重置管理员密码)。SQLite 需先停机；该命令撤销 Console 会话，不重置 Key 或财务数据。

## 监听器与请求体限制

参见[配置与容量](production-configuration.md#监听器与请求体限制)。JSON 和 multipart Images edit 使用独立上限，不能为图片上传提高全局 JSON 限制。

## 上游超时

参见[超时与运行时策略](production-configuration.md#上游超时)。TOML 的转发策略仅首次初始化数据库设置，后续在 Console 修改；接入显式超时优先。

## 公共数据面

参见[调用公共数据面](data-plane.md)：路由清单、字段白名单、压缩、重试、图片和独立搜索。六种操作使用独立路由，不跨格式转换或回退。

### 模型规则路由层级

参见[配置模型与路由](routing.md)：凭证 → 接入 → 逻辑渠道 → 操作能力 → 显式候选，以及固定渠道授权、健康测试与 Session affinity。

### Codex OAuth Connect

先[安装连接器插件](connector-plugins.md)，再按[Codex 接入指南](codex.md)配置账号、渠道、能力、模型路由和 Key 授权。导入 Token 不自动创建路由。

### Responses WebSocket

参见 [Responses WebSocket](websocket.md)：三层开关、顺序请求、连接隔离、`426` 回退、增量状态恢复和停机排空。

## Console 认证

参见[Console 认证](console.md#console-认证)：JWT、轮换 Cookie、邀请和注册。`admin` 是角色，不是另一套静态管理凭据。

### 管理员辅助密码恢复

参见[临时密码流程](console.md#协助用户恢复密码)：管理员确认后签发一次性显示、24 小时有效的临时密码，用户完成受限改密后才能使用普通 Console 功能。

## 普通用户接口

参见[普通用户接口](console.md#普通用户接口)：个人资料、会话、Key、日志及 Codex 额度可见范围。自助 Key 目标必须受有效 Policy 允许。

## 管理员接口

参见[管理员接口](console.md#管理员接口)；精确请求、响应与并发语义以 [Console OpenAPI](../openapi/console-v1.yaml) 为准。

## 自动禁用与定时测试

参见[健康测试与恢复](routing.md#管理故障转移自动禁用与定时测试)。定时测试会调用上游并记账，不是无成本健康检查。

## Session 粘性

参见[Session 粘性](routing.md#session-粘性)。缓存仅在当前进程，不保证跨实例上下文，也不允许重放已经发送的生成请求。

## 日志、用量与结算

参见[日志与监控](observability.md)及[计费与额度](billing.md)。独立计量事实、日志投影和结算各自推进；日志可见不等于已结算，日志缺失不等于无费用。

## 已知边界

支持范围见[公共数据面边界](data-plane.md#已知边界)。服务不终止 TLS，不提供跨实例限流/健康/粘性协调，也不提供充值、退款或货币兑换产品。安全配置与备份责任见[安全指南](security.md)。
