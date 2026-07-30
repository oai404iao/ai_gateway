# ip-api.com 代理出口 IP 查询

> 类型：外部参考
> 最近核对：2026-07-30
> 权威来源：[JSON API 文档](https://ip-api.com/docs/api:json)、
> [使用条款](https://ip-api.com/docs/legal)

## 外部接口关键语义

- 免费 JSON 接口为 `http://ip-api.com/json/`。不提供查询 IP 时，返回发起请求的出口 IP；
  `query` 字段承载该 IP，`fields` 参数用于限制响应字段。
- 免费接口只允许非商业用途且不提供 HTTPS。商业使用或 HTTPS 需要使用 ip-api.com 的付费服务；
  `ai-gateway` 不代替部署者取得相关授权。
- 免费接口按请求来源 IP 限流。响应 Header `X-Rl` 表示当前窗口剩余请求数，
  `X-Ttl` 表示限流窗口重置前的秒数；当 `X-Rl` 为 `0` 时，调用方必须等待窗口重置。

## ai-gateway 兼容行为

- 管理员通过 `POST /console/v1/network/proxies/test` 提交当前代理草稿。网关固定向
  ip-api.com 发起一次 JSON 查询，并通过该代理连接，因此返回的 `ip` 是代理出口 IP。
- 测试支持 HTTP、HTTPS、SOCKS4/4a 和 SOCKS5/5h 代理，复用与数据面相同的代理 URL
  与凭据校验规则。测试客户端位于独立的有界缓存中，不挤占正常转发客户端。
- 测试刻意忽略代理的 `enabled` 和 `no_proxy_hosts`，避免因 `ip-api.com` 命中绕过规则而
  错把 Gateway 自身出口地址显示为代理出口。
- 编辑已有代理时，省略的用户名或密码只会在代理 scheme、host 和有效 port 未改变时复用。
  更换代理端点后必须重新输入隐藏凭据，避免把旧凭据发送给另一个主机。
- 响应正文最多读取 64 KiB，连接、响应头和流空闲超时使用当前系统上游默认值。
  当 ip-api.com 返回 `X-Rl: 0` 或 HTTP `429` 时，同一诊断客户端会在 `X-Ttl`
  指示的窗口内拒绝重复测试。

## 差异与限制

- 测试结果仅用于人工诊断，不参与渠道健康、自动禁用、选路或计费。
- 免费端点使用明文 HTTP，结果可能被代理或链路中的其他参与方修改；不能把国家、ISP、
  `proxy` 或 `hosting` 标记作为安全授权依据。
- ip-api.com 的可用性、字段覆盖率、地理定位准确度和限流策略属于外部服务边界。
  外部服务不可用时，Console 返回受限的 `502`、`504` 或 `429` 错误，不回显代理凭据或
  上游原始错误正文。
