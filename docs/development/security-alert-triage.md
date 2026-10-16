# 安全告警核查与处置

> 状态：当前处置流程与历史核查记录。下列提交、行号和告警状态固定于各次核查基线，
> 不代表本次重查 GitHub 实时状态，也不构成完整安全审计。

安全告警按具体数据流和权限边界逐条处理。先判断真实修复、合成测试用途还是模型误报，
再记录决定与验证；不能用一次扫描成功或旧 dismissal 自动关闭新告警。

## 核查一条告警

1. 固定告警编号、查询、analysis、提交 SHA 和 SARIF source/sink，读取当时源码。
2. 沿数据流检查输入是否真的由 HTTP 客户端控制、是否经过范围/大小/权限屏障，以及 sink
   是否确实执行危险操作。不要仅凭变量名或 `State` 类型推断可控性。
3. 需要修复的提交 PR 并运行相关回归，等默认分支新扫描标记 `fixed`。
4. 仅对证据充分的测试用途或误报逐条 dismissal，写明审查提交和边界，不使用宽泛 `won't fix`。
5. 回读告警页的 reason/comment 或新 analysis，确认实际状态；API 写入成功不是完成证据。

本页其余部分：[处置边界](#处置边界)、[2026-09-09 基线](#2026-09-09-核查基线)、
[SDK 指针记录与重新评估](#验证与重新评估)。

## 处置边界

- Dependabot 告警通过升级依赖、更新锁文件及验证构建解决，不以误报关闭代替升级。
- CodeQL 告警必须结合具体 source、sink、权限和边界检查核查；扫描 workflow 成功不代表
  没有安全告警。
- 合成测试凭据使用 `used in tests`，错误的数据流或 sink 建模使用 `false positive`。
  每条 dismissal comment 保留审查提交和具体依据，不使用宽泛的 `won't fix`。
- 真正需要修改的代码由 PR 修复，等待默认分支重新扫描标记 `fixed`，不手动 dismissal。
- 不停用 Rust CodeQL、不排除整个测试目录、不全局屏蔽查询规则，也不改变生产鉴权或网络策略
  来迎合误报。后续告警仍需独立核查。

## 2026-09-09 核查基线

审查的是
[`e5e5917fad24397e169f7cf85fe1b10d6165e2f6`](https://github.com/oai404iao/ai_gateway/commit/e5e5917fad24397e169f7cf85fe1b10d6165e2f6)
的默认分支告警及 SARIF：CodeQL 2.26.4，Rust analysis `1745087514`，查询集
`security-extended`。该次扫描有 26 个结果，其中 #1–#5 已因测试用途关闭，
本次核查其余 21 条。下表是审查决定；实际状态以各告警页为准。

### 测试凭据：12 条

这些值仅用于测试密码校验、临时密码状态机、会话撤销或权限边界；不是生产默认密码，
不来自环境凭据。集成测试使用随机名称的临时 PostgreSQL 数据库；#9 在
`src/application/auth.rs` 的 `#[cfg(test)]` 模块内。决定均为 `used in tests`。

| 告警 | 基线位置 | 具体测试用途 |
| --- | --- | --- |
| [#9](https://github.com/oai404iao/ai_gateway/security/code-scanning/9) | `src/application/auth.rs:1098` | 短密码必须被哈希入口拒绝 |
| [#10](https://github.com/oai404iao/ai_gateway/security/code-scanning/10) | `tests/console_spec_integration.rs:677` | 紧急管理员密码重置的旧测试密码 |
| [#11](https://github.com/oai404iao/ai_gateway/security/code-scanning/11) | `tests/console_spec_integration.rs:678` | 同一重置用例的新测试密码 |
| [#12](https://github.com/oai404iao/ai_gateway/security/code-scanning/12) | `tests/console_spec_integration.rs:764` | 临时密码强制改密流程的旧测试密码 |
| [#13](https://github.com/oai404iao/ai_gateway/security/code-scanning/13) | `tests/console_spec_integration.rs:765` | 临时密码流程的新永久测试密码 |
| [#14](https://github.com/oai404iao/ai_gateway/security/code-scanning/14) | `tests/console_spec_integration.rs:1049` | 过期临时密码用例的账户初始化 |
| [#15](https://github.com/oai404iao/ai_gateway/security/code-scanning/15) | `tests/console_spec_integration.rs:1114` | 过期改密会话必须拒绝的新测试密码 |
| [#16](https://github.com/oai404iao/ai_gateway/security/code-scanning/16) | `tests/console_spec_integration.rs:1137` | 并发改密用例的账户初始化 |
| [#17](https://github.com/oai404iao/ai_gateway/security/code-scanning/17) | `tests/console_spec_integration.rs:1184` | 并发改密的第一个候选测试密码 |
| [#18](https://github.com/oai404iao/ai_gateway/security/code-scanning/18) | `tests/console_spec_integration.rs:1185` | 并发改密的第二个候选测试密码 |
| [#19](https://github.com/oai404iao/ai_gateway/security/code-scanning/19) | `tests/control_plane_integration.rs:1380` | 集成测试 seed 创建的合成登录密码 |
| [#20](https://github.com/oai404iao/ai_gateway/security/code-scanning/20) | `tests/console_spec_integration.rs:2401` | Codex quota 只读权限用例的 viewer 测试密码 |

### 数据流、分配及日志：9 条

| 告警 | 基线位置 | 证据与决定 |
| --- | --- | --- |
| [#6](https://github.com/oai404iao/ai_gateway/security/code-scanning/6) | `src/transforms/mod.rs:687` | sink 是 `Vec::insert` 的整数索引，不是日志。SARIF 将渠道构造值传播到数组索引；没有日志调用。`false positive` |
| [#7](https://github.com/oai404iao/ai_gateway/security/code-scanning/7) | `src/transforms/mod.rs:697` | sink 是 `Vec::remove` 的整数索引，不是日志。索引检查在操作之前。`false positive` |
| [#8](https://github.com/oai404iao/ai_gateway/security/code-scanning/8) | `tests/control_plane_integration.rs:6896` | 失败消息插入合成 fixture 值。修改为不带该值的固定消息，保留审计不泄露的断言；由扫描确认 `fixed` |
| [#21](https://github.com/oai404iao/ai_gateway/security/code-scanning/21) | `src/models_dev/mod.rs:52` | URL 由启动 TOML 的 `ModelsSyncConfig` 创建并保存在服务内。HTTP 请求只提供 provider/model 选择，不能设置 URL；client 禁止重定向。SARIF 将 Axum `State` 内的 client 标记为用户输入。`false positive` |
| [#22](https://github.com/oai404iao/ai_gateway/security/code-scanning/22) | `src/application/proxy_test.rs:161` | 生产目标固定为 `IP_API_ENDPOINT`；可替换目标的构造器用于本地集成测试。HTTP DTO 不含目标 endpoint，`State` 不由客户端填写。管理员可配置出站代理是已有权限内功能，不应改成禁止私网代理。`false positive` |
| [#23](https://github.com/oai404iao/ai_gateway/security/code-scanning/23) | `src/application/proxy.rs:1831` | 报告的初始分配是 `max_bytes.min(64 * 1024)`，最多 64 KiB；解压读取有 `take(max_bytes + 1)` 和长度检查，`max_bytes` 来自进程配置。`false positive` |
| [#24](https://github.com/oai404iao/ai_gateway/security/code-scanning/24) | `src/persistence/codex.rs:1229` | 批量凭据操作在分配前拒绝空输入及超过 100 项的输入。`false positive` |
| [#25](https://github.com/oai404iao/ai_gateway/security/code-scanning/25) | `src/persistence/mod.rs:5405` | 批量用户操作在分配前拒绝超过 100 项的输入，并检查唯一 ID。`false positive` |
| [#26](https://github.com/oai404iao/ai_gateway/security/code-scanning/26) | `src/persistence/mod.rs:5512` | 批量渠道操作在分配前拒绝超过 100 项的输入，并检查唯一 ID。`false positive` |

报告中的行号固定于审查基线，而不是随代码移动的永久定位。
`batch_mutations_reject_empty_and_oversized_inputs` 另外回归三类批量操作对空输入及
101 个唯一 ID 在任何数据库查询前的拒绝；使用已中止的临时数据库事务，避免不存在的
目标记录产生同类校验错误而掩盖上限检查回归。原有合法批量操作测试继续保留。

## 验证与重新评估

重新评估由边界变化触发，而不是把历史结论按行号套到新代码：

| 变化 | 需要重查的判断 |
| --- | --- |
| endpoint 开始接受 HTTP 输入、增加重定向或放宽权限 | URL 与 SSRF 可控性 |
| 移除大小/数量检查，或检查移到分配之后 | 资源耗尽和批量操作上限 |
| 合成密码进入生产配置 | 测试用途 dismissal |
| 数组操作改成日志输出 | source/sink 建模结论 |
| FFI 对象生命周期或卸载改变 | 指针存活与 nullable 入口契约 |

### 2026-10-08：连接器 SDK 描述符

PR #186 在 `e5a741fa71080d07ed1e2c34dc42a83f5fed3d8f` 的 Rust 扫描报告
[#52](https://github.com/oai404iao/ai_gateway/security/code-scanning/52)：
`crates/connector-sdk/src/lib.rs` 的单元测试解引用导出描述符。
独立审查确认宏内 `static DESCRIPTOR: OnceLock<PluginDescriptor>` 在进程生命周期内持有对象，
`get_or_init` 返回其引用，闭包只复制 raw pointer；manifest 字节由 `Box::leak` 持有。
没有栈对象返回、提前释放或动态卸载；解引用前的非空 `assert!` 排除了初始化失败的空指针。
SARIF analysis `1914524326` 的 source 实际是宏的 `null()` fallback，
而不是已经释放的分配；数据流遗漏了上述非空断言这一屏障。
因此按 `false positive` 逐条处置，不排除 FFI 或测试查询。

[#53](https://github.com/oai404iao/ai_gateway/security/code-scanning/53) 指向 SDK Responses
示例的同类指针。虽然存储同样为 static，示例此前未检查入口约定的 null-on-panic 返回。
两个测试最终都用 `as_ref().expect(...)` 将入口的 nullable pointer 显式转换为有效引用，
由后续扫描判断 #53 是否修复；不将原来缺少检查直接按误报关闭。
以后若描述符改为实例分配、支持卸载、移除断言或改变入口失败契约，必须重新评估。

- 测试日志改动运行 Rust 格式、Clippy、完整测试和 CodeQL；不修改生产转发路径，
  不为这次日志文案调整发起付费真实上游调用。
- 合并后核对 #8 是否被新默认分支扫描标记为 `fixed`，并读取其余告警的
  `dismissed_reason`、`dismissed_comment`；不能仅凭 API 写入成功就声称处理完毕。
- 修改误报模型时应先用最小复现验证；本记录不授权全局排除该类查询。

扫描和 CI 配置见[持续集成与安全扫描](continuous-integration.md)，真实转发改动的验证要求见
[真实上游 smoke](real-upstream-smoke.md)。
