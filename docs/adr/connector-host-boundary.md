# 宿主持有传输，插件生成协议计划

> 状态：已采纳。

**问题。** 用户请求的操作决定校验与选路；渠道连接器决定供应商如何认证、构造协议消息。二者并非同一个扩展点。若由插件自行发送请求，宿主就不能在一次逻辑请求中统一约束最后发出的地址和 Header，也难以判断断连时能否安全重试。

**考虑过的做法。** [new-api 的 channel adaptor](https://github.com/QuantumNous/new-api/blob/3a407cc9b064fe79a4ffac4146b2690c465dc929/relay/channel/adapter.go) 把格式转换和发送放在适配器接口内；[magpie 的 provider 插件](https://github.com/yetone/magpie/blob/9768b384e4d813f809aec5e23dd90292bfc00bf7/docs/subsystems/provider-plugins.md) 把请求交给独立插件宿主。前者方便供应商定制但会分散网络副作用，后者提供进程边界但引入 IPC、取消、背压与发送结果不确定性。

**选择。** 保持单进程，用独立 SDK 定义 C ABI 和协议，用 connector-host 加载受信制品。应用固定一次请求所用的插件代际，校验插件返回的目标、消息与响应适配结果，再自行派发、计量和记录。普通字段变换仍由受限 DSL 表达，不为每个供应商复制一条转发流水线。

**代价。** 编译边界不是故障或秘密隔离：原生代码与宿主同权。若必须运行不可信插件，单靠收紧命令 schema 不够；必须重新设计跨进程隔离和派发确认。当前职责见[转发边界](../development/relay-boundaries.md)及[插件契约](../development/connector-plugins.md)。
