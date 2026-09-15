# 控制面软删除

> 状态：当前。三个阶段已覆盖身份、授权、普通渠道、渠道组和计价模型。

## 目标与边界

控制面资源使用不可恢复的墓碑式软删除，以保留请求日志、结算和审计引用。普通列表、详情、
运行时快照及可选资源列表都只读取 `deleted_at IS NULL` 的记录。删除后的自然名称可以复用，
但 UUID 永不复用。

软删除不提供回收站、恢复或物理清理 API。请求日志、审计日志、系统设置、会话事实和结算事实
不属于可删除资源。

## 三阶段计划

### 阶段一：身份与授权资源

- 沿用用户匿名化软删除，并将其普通 API Key 一并转为墓碑。
- 为用户组和 API Key 增加 `deleted_at`、`deleted_by`、活动记录部分唯一索引及硬删除保护。
- 删除自定义用户组时，按角色把现有成员迁移到内置默认组，禁用关联注册码，并移除 Codex
  quota 可见性关系。
- 同时提供管理员和 Key 所有者的 API Key 删除 API；删除会撤销 Key 并擦除明文 secret。
- Console 为用户、用户组和自有 API Key 提供不可恢复删除确认。

### 阶段二：渠道与渠道组

- 普通 OpenAI-compatible 渠道和渠道组使用墓碑。
- 删除渠道组时软删除其渠道，并从模型路由、API Key、API Key Policy 和 quota 可见性中自动解绑。
- 受影响的显式渠道/模型候选、空 tier 和空协议规则会自动规范化。
- 删除前使用权威影响预览；影响变化时要求管理员重新确认。
- Codex 托管渠道及 connector pool 生命周期不复用普通删除路径。

### 阶段三：模型与收尾

- 为价格模型增加墓碑，清除渠道定时测试引用，并禁用、隐藏其路由 profile 下的协议规则。
- 允许以相同 `source_model_id` 创建新的 UUID，不复活旧墓碑。
- 完成跨资源前端一致性、历史查询验证、操作文档和端到端删除流程。

第二期再评估 API Key Policy、代理、转换模板、独立模型协议规则、注册码和 Codex connector
pool 的删除能力。Codex 拼车组不提供删除，避免形成重置金额账本的路径。

## 阶段一语义

### 用户

用户删除继续清除 email、密码和临时密码，撤销 Session 与邀请，并迁移到对应的内置默认组。
所属普通 API Key 会同时标记删除、设为 `revoked` 并擦除明文 secret。删除后的用户和 Key UUID
仍可被请求日志、结算和审计引用。管理员不能删除自己或最后一个活动管理员。

Codex 拼车席位保留原用户 UUID 作为历史成员；数据面只承认活动用户，因此墓碑用户不能继续使用
席位，管理员仍可按现有席位替换规则安排新成员。

### API Key

撤销和删除是两个动作：

- 撤销只设置 `status = 'revoked'`，记录仍显示在管理列表中。
- 删除同时设置 `deleted_at`/`deleted_by`、撤销 Key、用唯一墓碑值覆盖 `secret_value`，并从普通
  API 隐藏。

系统探测 Key 不允许通过 Console 删除。删除使用详情响应的 `ETag`/`If-Match`。

### 用户组

内置 `user`/`admin` 默认组受保护。删除自定义组时，在同一串行化事务中：

1. 普通用户迁移到内置用户组，管理员迁移到内置管理员组。
2. 关联的可复用注册码设为禁用，但保留其历史组引用。
3. 删除该组的 Codex quota 可见性关系。
4. 写入组墓碑并发布完整运行时快照。

用户现有 API Key 的路由目标快照不变；组迁移可能改变 Key 创建策略和 Fast 模式过滤，因此删除
确认必须明确提示这一影响。

## 一致性与安全

删除和自动处理依赖均在现有 `SERIALIZABLE` 控制面事务中执行。事务完成候选运行时配置编译、
审计写入和提交后，才通过 `ArcSwap` 发布新快照。旧快照中的在途请求可以结束并继续结算；
新请求不能认证已删除的用户或 Key。

明文 API Key 和上游渠道凭据不进入删除影响或删除后的审计快照。直接 SQL `DELETE` 由数据库
触发器拒绝，避免绕过墓碑和历史外键。

## 阶段二语义

### 普通渠道

管理员先读取 `GET /console/v1/routing/channels/{id}/deletion-impact`，再用详情 `ETag` 和预览返回的
`confirmation_token` 调用 `DELETE /console/v1/routing/channels/{id}`。删除事务会：

1. 移除该渠道的全部显式渠道/模型候选。
2. 删除因此为空的 tier；协议规则失去最后一个 tier 时自动停用。
3. 从未删除 API Key 和 API Key Policy 的显式渠道数组中移除该 UUID。
4. 停用渠道，清除自动禁用状态、上游 URL、代理、超时、转换模板、渠道转换、上游凭据、模型能力和
   定时测试引用，再写入墓碑。

渠道名称在同一组中可以由新的 UUID 复用。请求日志和审计事实仍按旧 UUID 读取墓碑；普通列表、
详情、运行时快照、定时测试和 API Key 可选项均隐藏旧记录。

### 普通渠道组

渠道组使用对应的 `/deletion-impact` 与 `DELETE` 接口。删除会先对组内全部普通渠道执行上述墓碑
处理，再移除这些渠道的模型路由候选、API Key/API Key Policy group 与 child-channel 引用，以及
匹配的 quota 可见性关系。组级状态监控同时关闭，空 tier 和空协议规则按同一规则规范化。组名可由
新的 UUID 复用。

Codex OAuth 组及其 Responses/Images managed channels 返回
`provider_managed_resource`，必须继续通过凭证和 connector pool 的专用生命周期管理，不能借普通
删除路径重置身份、quota 或共享账本。

### 权威影响确认

影响预览列出将成为墓碑的渠道、路由将变化的协议规则、需解绑的 API Key/API Key Policy，以及将
删除 quota 可见性的用户组。`confirmation_token` 对当前资源版本和上述依赖快照取指纹。删除事务在
`SERIALIZABLE` 隔离级别中重新计算；依赖新增、移除或变化后，旧 token 返回
`409 deletion_impact_changed`，Console 获取新预览并要求再次确认。根资源自身的并发修改仍由
`If-Match` 独立检测。

## 阶段三语义

### 计价模型

管理员通过 `DELETE /console/v1/models/{id}` 和详情 `ETag` 进行不可恢复删除。串行化事务先停用该
模型 routing profile 下全部已启用协议规则，再清除所有 Channel 成对设置的 `test_model` /
`test_pricing_model_id`，最后停用模型并写入 `deleted_at`/`deleted_by`。Profile、协议规则、routing
tier 和 candidate 行继续保留，已有 `request_logs.model_id` / `model_rule_id` 与审计引用因此保持有效。

普通模型列表、详情、models.dev 同步匹配、运行时价格表和协议规则查询只读取活动模型。墓碑的
`source_model_id` 由部分唯一索引释放；手工创建或目录导入同名模型会得到新 UUID，不会更新或恢复
旧墓碑。新协议规则和 Channel 定时测试不能引用墓碑模型，数据库触发器同时拒绝恢复、修改或硬删除
模型墓碑。

删除不移除 Channel 的 `available_models`。该字段描述上游 wire model 能力，不等同于
`test_pricing_model_id` 指向的计价身份，也可能仍被其他活动模型的 route candidate 使用。

### 历史查询与 Console

请求日志继续使用请求发生时保存的客户端/上游模型字符串、计价模型 UUID、协议规则 UUID 和价格
快照，因此模型删除或同名 UUID 重建不会改写历史日志、统计或结算。活动目录不会为了展示历史事实
重新暴露墓碑。

管理员详情页统一使用同一危险操作区和不可恢复确认对话框。模型确认会明确提示协议规则停用、定时
测试解绑和历史保留；用户、用户组、普通渠道及渠道组继续显示各自的保护条件或权威影响预览。

## 验证

每个阶段需要覆盖 migration、仓储、Console 契约、组件和浏览器流程。阶段一重点验证：

- 删除用户、用户组和 API Key 后，列表、详情、运行时快照及更新操作都看不到墓碑。
- 用户组成员按角色迁移，注册码自动禁用。
- Key secret 已被覆盖，原 secret 立即无法认证，名称可以重新使用。
- 删除前已经写入或在途产生的请求日志仍可正常结算。
- 系统记录、当前管理员和最后一个活动管理员保护不变。

阶段二重点验证：

- 普通渠道/渠道组删除后从列表、详情、运行时快照、定时测试和可选项消失，自然名称可复用。
- 渠道组的所有 child channels 都写入非敏感墓碑，上游凭据和转换配置已清除。
- 路由渠道/模型候选、空 tier 和空协议规则按预览结果规范化；失去最后一个 tier 的协议
  自动停用。
- API Key、API Key Policy 和 quota 可见性引用自动解绑，旧影响 token 不会执行删除。
- Codex managed channel/group 与直接 SQL 硬删除保护不变。

阶段三重点验证：

- 模型删除后从 Console 列表、详情、模型规则和运行时快照消失，全部协议规则保留但已停用。
- 所有 Channel 的匹配定时测试计价引用成对清空，且不能重新引用墓碑模型。
- 原 `source_model_id` 可创建新 UUID；models.dev 同步不会更新旧墓碑。
- 历史请求日志继续返回原模型、协议规则及渠道事实，审计保留删除 actor 和自动处理摘要。
- 模型墓碑不可恢复、修改或硬删除，Console 组件与浏览器流程均要求不可恢复确认。

相关来源：

- [数据库与控制面架构](database-architecture.md)
- [Console API 契约](../openapi/console-v1.yaml)
- [Codex 拼车金额账本](codex-sharing.md)
