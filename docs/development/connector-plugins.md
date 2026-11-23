# 原生连接器插件

> 状态：当前。

`general` 由网关实现；Codex 由独立的 [ai-gateway-connectors](https://github.com/oai404iao/ai-gateway-connectors) 仓库构建。管理员[安装并启用制品](../user/connector-plugins.md)后，网关可以将渠道绑定到插件。安装、启用、路由和访问授权是不同操作。

## 调用与职责

宿主按 [C ABI 1 和命令协议 3](../../crates/connector-sdk/docs/commands.md) 调用动态库。同步调用分别传 JSON 元数据和原始 body，由分配方释放内存。宿主依据 `attempt.describe/v1` 发布操作支持的 transport、响应方式与用量解析器；这些描述只能收窄渠道能力。Codex 响应透传，使用实际 Responses 上游的[用量口径](usage-normalization.md)。通用插件还可以声明[同格式响应适配](connector-response-adapters.md)。

宿主执行公共请求策略、选路、鉴权、网络、数据库事务与结算。插件在管理员变换之后构造供应商请求，处理身份、隐私、OAuth、模型和额度等供应商协议。宿主验证插件结果和最终出站请求，再派发网络调用；插件准备失败就结束该尝试。

## 安装与加载

`[plugins].directory` 是进程独占的私有目录。`incoming/` 接收完整 `.tar.gz`（投递方从 `.partial` 原子改名）；`staging/` 暂存上传；`artifacts/<id>/<digest>/` 存储不可变制品和许可证。目录投递与网页上传走同一安装器。重启后中断的安装任务标记为失败，重新安装需再次提交制品。

宿主校验包成员、大小、文件权限、平台和摘要，再从 sealed memfd 加载刚验证的字节。摘要证明内容相同，不证明发布者可信：**原生插件以网关进程权限执行**。停用不会卸载已映射的库；达到进程加载上限需要维护重启。删除制品写入持久墓碑，周期扫描不会把已删制品重新登记。

## 设置、快照与在途请求

`plugin_artifacts` 记录库存，`plugin_states` 选择活动制品，`plugin_settings` 保存插件定义的设置，`plugin_install_jobs` 跟踪任务。管理员变更走 Console 一次性再认证。设置 ETag 绑定设置 revision 与所选摘要；启停使用状态 revision。插件提供设置描述、校验和编译，编译结果作为不可变 `settings` 随调用传入。设置 schema 的显式迁移同样经过校验和编译，失败时保留现有设置。

宿主在控制面事务中编译候选快照，提交后原子发布。不可用插件的渠道不可派发，其他渠道和管理 API 仍可用。新请求使用新代际，在途请求持有原实例；WebSocket 的池键隔离代际，连续请求的 `previous_response_id` 需要命中原连接。OAuth state 同样绑定代际，期间切换插件或设置需要重新开始授权。维护操作仍按后端的锁或耐久 intent 结算，不因插件切换而重放外部请求。

## 验收边界

在 Linux 构建[固定制品](../../tests/fixtures/codex-plugin.json)，生成的动态库用于真实宿主加载和回环转发验证。配置隔离测试库的方法见[开发环境](getting-started.md)。

```sh
export AI_GATEWAY_TEST_CODEX_PLUGIN="$(./scripts/prepare-connector-tests.sh)"
cargo test --locked -p ai-gateway-connector-host -- --include-ignored
cargo test --locked --test control_plane_integration \
  codex_connector_forwards_responses_and_images_with_shared_credentials -- --exact
```
