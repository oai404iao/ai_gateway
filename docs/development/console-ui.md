# Console Web UI 设计与实施计划

> 状态：已完成设计记录。React + TypeScript + Vite + Tailwind +
> shadcn/ui（Radix）工程位于 `web/console/`，可选
> `embedded-console-ui` Cargo feature 将 `web/console/dist` 编入二进制并由
> Console listener 同源提供。当前实现以 `web/console/`、`src/http/console_ui.rs`
> 和测试为准。

## 1. 目标与边界

本计划为现有 JWT 认证的 Console API 提供浏览器管理界面，用于用户自助服务和管理员控制面操作。

- UI 面向 Console 用户和管理员，不是面向最终用户的聊天产品，也不是可嵌入第三方网站的 Widget。
- UI 只能由 Console listener 提供，绝不挂到公共数据面 listener 或 `/v1/*` 路径。
- 数据面仍只接受 OpenAI 兼容客户端请求和客户端 API Key；浏览器 UI 不改变代理、选路、流式转发或运行时快照边界。
- Rust 服务仍是唯一的生产运行时；Node.js、Vite 和 pnpm 仅参与前端开发、测试与构建。
- 前端源码独立组织，发布产物可选择性编入 Rust 二进制，以保持单交付物部署体验。

当前 API 与认证边界的实现位于 `src/http/console.rs`，启动时在 `src/main.rs` 上创建独立
Console listener。实施 UI 前必须保持这两个监听器及其权限模型的隔离。

## 2. 已确定的架构决策

| 主题 | 决策 |
| --- | --- |
| 前端形态 | React + TypeScript 单页应用（SPA），使用 Vite 构建静态资源。 |
| 组件体系 | Tailwind CSS + shadcn/ui，选择 **Radix** primitives；shadcn 组件作为受版本控制的项目源码，而非黑盒 UI 依赖。 |
| 发布形态 | Vite 先构建 `dist/`，启用 `embedded-console-ui` Cargo feature 时由 `rust-embed` 编入二进制。 |
| 生产访问方式 | UI 与 `/console/v1/*` API 在同一个 HTTPS origin 下运行，例如 `https://console.example.com/`。 |
| 开发访问方式 | Vite 开发服务器通过同源反向代理转发 `/console/v1/*`；不以开放 CORS 作为日常开发方案。 |
| 服务端渲染 | 不采用 SSR、Next.js 或常驻 Node 服务。Console 已有完整 JSON API，SSR 不值得增加第二个生产运行时。 |
| UI 路由 | React Router 管理浏览器路由和嵌套页面布局；TanStack Query 是唯一的 Console HTTP 服务端状态缓存。 |
| API 类型 | 以一份受版本控制的 Console OpenAPI/契约文档生成 TypeScript 类型；禁止在各 feature 中散落复制请求/响应形状。 |

Vite 产物是可静态托管的应用 bundle；shadcn/ui 支持 Vite 项目，并通过 CLI 将组件源码加入项目。
`rust-embed` 可按目录读取嵌入资源，适合将已构建的静态产物纳入发布二进制。实现时以各项目的锁文件
和上游文档为准，不在本文固定依赖版本。

## 3. 运行拓扑与 URL 规则

```text
浏览器
  │ HTTPS  https://console.example.com
  ▼
TLS 反向代理
  │
  └──> Console listener（现有独立端口，例如 127.0.0.1:3001）
        ├── /console/v1/*    现有 JWT Console API
        ├── /assets/*        Vite 指纹静态资源
        ├── /favicon.* 等    明确的静态根资源
        └── 其他 HTML 导航   SPA index.html fallback

OpenAI 客户端
  │
  └──> 公共 listener（现有端口，例如 127.0.0.1:3000）
        └── /health、/v1/*
```

必须遵守下列路由规则：

1. `/console/v1/*` 必须先匹配 API 路由，API 的 404、405、认证错误和 JSON 错误响应不得退化为
   `index.html`。
2. SPA fallback 只处理 `GET` / `HEAD` 的 HTML 导航请求，且不得处理 API 前缀。
3. UI 路由的基础路径固定为 `/`；API 固定保留 `/console/v1`，避免可配置子路径导致构建资产、Cookie
   与反向代理规则不一致。
4. 公共 listener 不提供 UI、静态资源或 SPA fallback。现有
   `public_router_does_not_expose_console_paths` 测试应继续成立。
5. 生产反向代理负责 TLS；应用二进制继续不终止 TLS。

同源部署是安全与运维上的默认选择：浏览器只需使用同一个 origin 请求 API，既不需要为正常生产访问
开放 CORS，也不会让 refresh Cookie、预检与跨域凭据成为额外的运行变量。

## 4. 身份认证、授权与浏览器安全

现有认证模型保持不变，前端只适配它：

| 场景 | 前端行为 |
| --- | --- |
| 首次加载 | `SessionProvider` 调用 refresh，若成功则只在内存保存新的 access token 和用户资料。 |
| 普通 API 请求 | `ConsoleApiClient` 在内存 token 存在时加入 `Authorization: Bearer …`。 |
| 收到一次 401 | 对并发请求实施 single-flight refresh；刷新成功后仅重试原请求一次，失败则清空本地会话并跳转登录页。 |
| 登录/邀请激活/刷新 | 使用 `credentials: "include"`，让现有 `HttpOnly; Secure; SameSite=Lax` refresh Cookie 正常工作。 |
| 刷新页面 | access token 不持久化；应用重新走 refresh 流程。 |
| 登出、改密、禁用或角色变更 | 尊重后端 session 与 `auth_version` 失效结果，清空前端缓存并回到未认证状态。 |

具体约束：

- **不得**把 access token、refresh token 或 API Key 写入 `localStorage`、`sessionStorage`、URL、
  日志、错误上报 payload 或浏览器持久化查询缓存。
- 前端菜单可按 JWT 返回的 role 隐藏管理员入口，但这只是 UX；后端 `require_admin` 仍是唯一授权边界。
- 在 UI 上线前，评估并实现 cookie 驱动 refresh 端点的同源 `Origin` 校验或等价 CSRF 防护；不能仅因
  SPA 同源而删除现有 Cookie 安全属性。
- API Key 可由其所有者和管理员重新读取，但前端必须默认打码，并仅在用户主动点击后显示完整值；复制动作
  始终复制完整值。API Key 不得写入浏览器持久化存储、URL 或日志。
- 邀请 token 等真正的一次性 secret 必须使用专用“仅展示一次”对话框。关闭后不从查询缓存、路由 state
  或日志恢复。
- UI 静态响应至少添加 CSP、`X-Content-Type-Options: nosniff`、`Referrer-Policy` 与
  `frame-ancestors 'none'`。生产 CSP 仅允许本 origin 的脚本、样式、连接和图片，除非新增功能有明确
  安全评审。

## 5. 前端技术栈与使用约定

### 5.1 基础依赖

| 类别 | 选择 | 责任 |
| --- | --- | --- |
| 构建 | Vite | 本地开发服务器、生产静态构建与资产指纹。 |
| 视图 | React + TypeScript（严格模式） | 页面、组件和可访问性交互。 |
| 路由 | React Router | 登录/邀请页、已认证 Shell、嵌套路由、404 与路由级错误边界。 |
| 服务端状态 | TanStack Query | 列表、详情、mutation、失效、重试和乐观/悲观更新边界。 |
| 表单与校验 | React Hook Form + Zod | 表单状态、字段级错误和提交前 DTO 校验。 |
| UI | Tailwind CSS + shadcn/ui（Radix） | 可访问的基础组件、主题 token 和一致的管理界面。 |
| 表格 | TanStack Table（仅复杂表格） | 排序、过滤、列显示等复杂交互；简单展示继续使用 shadcn `Table`。 |
| 通知 | shadcn 推荐的 `sonner` | 成功、失败和后台刷新通知。 |
| 测试 | Vitest + React Testing Library + MSW；Playwright | 单元/组件、API 行为模拟和真实浏览器端到端验证。 |

使用 pnpm，并提交 `pnpm-lock.yaml`。`package.json` 必须声明 `packageManager`；CI 和开发机不依赖
全局安装的 Vite、shadcn 或其他前端 CLI。

### 5.2 shadcn/ui 约定

1. 初始化 Vite + React + TypeScript 后使用 `pnpm dlx shadcn@latest init`，选择 Radix base，
   配置 `@/* -> src/*` alias 和 Tailwind CSS。
2. 组件只通过 shadcn CLI 加入；添加前检查组件文档，更新已有组件时先运行 `--dry-run` 与 `--diff`，
   未经明确审查不得覆盖本地修改。
3. `src/components/ui/` 只存放 shadcn 管理的基础组件；业务组件放入对应
   `src/features/<feature>/components/`，跨业务的组合组件放入 `src/components/shared/`。
4. 表单使用 shadcn 的 `FieldGroup`、`Field`、`FieldLabel` 和受控输入组件；字段错误使用
   `data-invalid` / `aria-invalid`。JSON 变换编辑器的第一期可使用带 Zod/JSON 解析错误提示的
   `Textarea`，不先引入任意代码执行或未审计编辑器插件。
5. 优先使用组件已有的 variant、语义 Tailwind token、`cn()` 和 `gap-*` 布局；不以原始色值、
   随意 `dark:` 覆盖或自制不可访问弹层替代 shadcn 组件。
6. 删除、撤销、禁用等危险操作使用 `AlertDialog`；`Dialog`、`Sheet`、`Drawer` 均必须有可访问的
   标题；通知统一通过 `sonner`，不实现另一套 toast。
7. 外部/community registry 组件不是默认来源。使用前必须明确 registry、检查引入的依赖与源码，并修正
   import alias 后再提交。

第一批预计需要的基础组件包括：`button`、`card`、`input`、`textarea`、`select`、`checkbox`、
`switch`、`field`、`form`、`table`、`tabs`、`badge`、`sidebar`、`sheet`、`dialog`、
`alert-dialog`、`dropdown-menu`、`tooltip`、`pagination`、`skeleton`、`empty`、`spinner` 和
`sonner`。实际添加遵循按需原则，而不是一次性引入全部组件。

## 6. 代码组织

### 6.1 计划中的前端目录

```text
web/console/
  package.json
  pnpm-lock.yaml
  components.json
  vite.config.ts
  tsconfig.json
  tsconfig.app.json
  eslint.config.*
  index.html
  public/                         # 少量非指纹静态文件；默认避免放业务资源
  src/
    main.tsx
    app/
      router.tsx                  # 路由树、Shell、懒加载和路由错误边界
      providers/                  # QueryClient、Session、主题、全局错误边界
      layouts/                    # PublicLayout、ConsoleLayout
    api/
      client.ts                   # fetch 封装、Bearer、single-flight refresh、错误映射
      session.ts                  # login/refresh/logout transport
      generated/                  # 从 Console API 契约生成；禁止手改
      errors.ts                   # 可呈现的 API/网络/并发错误
    components/
      ui/                         # shadcn CLI 生成的基础组件
      shared/                     # PageHeader、DataTable、SecretOnceDialog 等组合组件
    features/
      auth/
      profile/
      api-keys/
      request-logs/
      users/
      api-key-policies/
      models/
      catalog/
      routing/
        channel-groups/
        channels/
        model-rules/
      network/
        proxies/
      transforms/
        templates/
      audit-logs/
      system/
      # 每个 feature 仅在本目录内放 api.ts、schemas.ts、queries.ts、
      # mutations.ts、components/、pages/ 与 feature 局部类型。
    lib/
      cn.ts
      dates.ts
      etag.ts
      formatters.ts
      permissions.ts
    styles/
      index.css                  # Tailwind 和 shadcn 主题变量的唯一全局入口
  tests/
    integration/
    e2e/
```

规则：

- 不创建“万能 `services/`”或“万能 `utils/`”目录；HTTP transport 在 `api/`，领域行为归各
  `features/`，通用且无领域含义的函数才放 `lib/`。
- 页面不直接调用 `fetch`，也不直接拼接 API JSON；页面只消费 feature 暴露的 Query/mutation hooks。
- Query key 由 feature 集中导出。控制面 mutation 成功后，按资源范围失效相关列表/详情，而不是全局
  `invalidateQueries()`。
- 所有可更新资源必须读取后端 ETag，并在 `PUT` 时带上 `If-Match`。收到 `409` 时显示“数据已被其他
  操作者修改”，提供重新加载而非静默覆盖。
- React Router 负责 URL、布局、导航与错误边界；Console HTTP 数据的缓存、重新获取和 mutation 状态
  全部由 TanStack Query 统一管理，避免双重缓存。

### 6.2 计划中的 Rust 变更边界

```text
src/
  http/
    console.rs                    # 保持 Console API、JWT、CORS 与 no-store API 语义
    console_ui.rs                 # 新增：嵌入资源、静态响应、HTML fallback、缓存/安全 Header
    mod.rs                        # 公共 router 继续不挂载 Console UI
  runtime_config/
    mod.rs                        # 新增 Console UI 开关的 TOML 解析与校验
  main.rs                         # 根据 feature + 配置组合 Console API 与 UI router
Cargo.toml                        # optional rust-embed/mime 依赖与 embedded-console-ui feature
config.example.toml               # 记录 Console UI 开关和部署说明
tests/
  console_ui_integration.rs       # 静态资源、fallback、缓存、安全和 API 隔离测试
web/console/                      # 前端源码（见上）
```

`src/http/console.rs` 目前为所有 Console API 统一加入 `Cache-Control: no-store`。实施时必须将
API router 与 UI router 分开组合：`no-store`、CORS 和 API body limit 只应用到 API，不能意外令带
hash 的 `/assets/*` 失去长期缓存，也不能让 UI 静态路由继承不必要的 API middleware。

## 7. API 契约、数据与交互规则

### 7.1 契约生成

受版本控制的 Console API 契约已落地，作为前端类型的单一事实来源：

```text
docs/openapi/console-v1.yaml       # 权威规范源文件（手写）
web/console/src/api/generated/console-v1.d.ts  # openapi-typescript 生成，禁止手改
web/console/src/api/types.ts      # 薄 shim：re-export 生成结果的 schema
```

契约覆盖登录、刷新、邀请、本人资源、管理员资源、错误响应、`ETag`/`If-Match`、分页/limit 参数、
可重新读取的 API Key、一次性邀请 token 和 role 权限。生成命令为 `pnpm --dir web/console generate:api`；
`pnpm --dir web/console generate:api:check` 重新生成并用 `git diff --exit-code` 校验无漂移，供 CI 使用。
`types.ts` 仅把 `components["schemas"]` 下的 schema 以同名导出，加上客户端聚合体 `ControlPlaneLists`；
页面继续从 `@/api/types` 导入，不直接依赖生成文件的内部结构。在 Rust 自动导出 OpenAPI 成熟前，
API 集成测试应同时验证实现与该规范的关键请求/响应示例，避免规范与实际 handler 漂移。

### 7.2 资源更新

- 读取详情时保存 ETag 到 feature 局部状态；编辑表单提交时从该状态生成 `If-Match`。
- 新建/撤销 API Key、邀请用户等返回 `secret` 或 token 的 mutation，不写入 Query Cache；只把安全的
  `id` 和资源元数据刷新到列表。
- 管理 mutation 成功后展示后端返回的 `correlation_id`，供排障和审计记录关联。
- request log、audit log 首期按照现有 `limit` 参数读取；若要支持大数据量浏览，先扩展后端为显式
  cursor 分页，不能仅在浏览器无限加载全量数据。
- 日期、金额、token 用量和空值使用统一 formatter；系统金额统一按 USD 显示，不提供币种设置或换算。

### 7.3 页面与权限范围

| 区域 | 建议浏览器路由 | 后端 API 范围 | 最低角色 |
| --- | --- | --- | --- |
| 登录/邀请激活 | `/login`、`/activate-invitation` | `/console/v1/auth/*` | 匿名 |
| 个人资料与安全 | `/account`、`/account/sessions` | `/console/v1/me*` | user |
| 我的 API Key | `/api-keys` | `/console/v1/me/api-keys*` | user |
| 我的请求日志 | `/usage/request-logs` | `/console/v1/me/request-logs*` | user |
| 用户与策略 | `/admin/users`、`/admin/api-key-policies` | `/console/v1/users*`、`/api-key-policies*` | admin |
| 模型和目录 | `/admin/models`、`/admin/catalog` | `/console/v1/models*`、`/catalog/models/*` | admin |
| 路由 | `/admin/routing/*` | `/console/v1/routing/*` | admin |
| 网络与变换 | `/admin/network/proxies`、`/admin/transforms/templates` | `/console/v1/network/*`、`/transforms/*` | admin |
| 可观测性与系统 | `/admin/request-logs`、`/admin/audit-logs`、`/admin/system` | `/console/v1/request-logs`、`/audit-logs`、`/system/reload` | admin |

浏览器路由中的 `/admin` 仅表示 UI 信息架构，不新增或恢复 `/admin/v1/*` 后端接口。

## 8. 嵌入、配置、缓存与发布

### 8.1 编译与运行时开关

当前使用两个明确开关：

```toml
# 当前配置
[console]
enabled = true
ui_enabled = true
host = "127.0.0.1"
port = 3001
allowed_origins = []
```

- `console.enabled`：Console listener 的总开关。
- `console.ui_enabled`：只控制是否在该 listener 挂载 UI；默认为 `false`，使 API-only 部署保持当前行为。
- `embedded-console-ui`：Cargo feature，控制二进制是否包含前端资源。
- 若 `ui_enabled = true` 但二进制未启用 feature，启动必须返回明确配置错误，不能静默提供一个空白或磁盘
  目录页面。
- 如果 feature 启用但 `ui_enabled = false`，资源可在二进制内但不暴露 HTTP 路由；用于同一发布包的
  受控启用场景。

不使用运行时可配置的 UI 目录，也不使用 `ServeDir` 指向部署主机的任意文件系统路径。这样可避免二进制
与静态资源版本错配、路径遍历风险和多机部署不一致。

### 8.2 HTTP 缓存与静态处理

| 路径 | 行为 | Cache-Control |
| --- | --- | --- |
| `/` 和 SPA HTML fallback | 返回 `index.html` | `no-cache` |
| `/assets/<fingerprinted-file>` | 返回 Vite 指纹资源 | `public, max-age=31536000, immutable` |
| 根目录非指纹资源 | 仅显式 allowlist | 保守 `no-cache`，除非文件名和构建策略证明可长期缓存 |
| `/console/v1/*` | 保持现有 API 行为 | `no-store` |

静态 handler 必须基于嵌入资源的固定相对路径进行查找，拒绝空路径以外的 `..`、反斜杠和不合法编码，
根据文件扩展名设置正确 `Content-Type`，并支持 `HEAD`。不要使用文件系统路径拼接来实现 SPA fallback。

### 8.3 构建流程

前端构建必须显式先于嵌入式 Rust 构建：

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build
cargo build --release --features embedded-console-ui
```

不得在 `build.rs` 中隐式安装依赖或调用 pnpm：这会让 `cargo test`、离线构建、交叉编译和纯后端贡献者
的工作流不可预测。建议提供一个明确的 `just`/`make`/脚本目标封装上述命令，但底层步骤仍保持可见。
`rust-embed` 的默认 debug 行为会从资源目录读取文件，因此默认 `cargo test` 不启用
`embedded-console-ui`；任何启用该 feature 的测试/构建都必须先准备好 `web/console/dist`。

开发期使用本地 HTTPS 的同源 Console hostname，例如
`https://console.localhost:5173`，由 Vite 代理 `/console/v1/*` 到本地 Console listener。这样不需要
为了开发而放宽生产 Cookie 属性或 CORS 策略。Vite 的 `preview` 只用于检查构建产物，不作为生产服务。

## 9. 分阶段实施计划

> 以下 checkbox 保留原始实施计划，不再作为当前完成度追踪器。

### Phase 0：契约与交付基础

- [x] 确认 Console API 的请求/响应、错误、ETag 与授权矩阵，新增 `docs/openapi/console-v1.yaml`。
- [ ] 创建 `web/console/` 的 pnpm + Vite + React + TypeScript 工程，初始化 Tailwind、shadcn/ui
  (Radix)、严格 TypeScript、ESLint、Vitest 和 Playwright。
- [ ] 设定 `.gitignore`：忽略 `node_modules/`、`dist/`、测试报告和本地浏览器工件；提交 lockfile 和
  `components.json`。
- [x] 添加 `generate:api`、`generate:api:check`、`typecheck`、`lint`、`test`、`build` 脚本；`generate:api:check` 供 CI 验证生成无漂移。

验收：前端能够在不连接生产服务时完成类型检查、lint、单元测试和静态构建；契约生成结果无漂移。

### Phase 1：嵌入式交付与认证 Shell

- [ ] 新增 `embedded-console-ui` feature、嵌入资源模块、`console.ui_enabled` 配置和明确的 feature/config
  校验。
- [ ] 将 Console API router 与静态 UI router 分开组合，保留 API 的 CORS、body limit 和 `no-store`。
- [ ] 实现静态资源 Content-Type、缓存、安全 Header、GET/HEAD、HTML fallback 和 API 排除规则。
- [ ] 实现登录、邀请激活、bootstrap refresh、single-flight token refresh、登出和全局未认证处理。
- [ ] 构建响应式 `ConsoleLayout`、role-aware 导航、错误边界、loading/empty/error 状态和深浅主题。

验收：以单个 release 二进制在 Console origin 加载 UI；刷新页面能恢复会话；公共 listener、API 错误和
未知资产不会错误返回 `index.html`。

### Phase 2：普通用户闭环

- [ ] 实现个人资料、密码、session 管理、本人 API Key 和本人请求日志页面。
- [ ] 实现 API Key 默认打码、主动显示、复制、撤销确认和 ETag 冲突处理。
- [ ] 为用户表单、会话撤销、API Key 创建/更新/撤销和日志详情补充组件/浏览器测试。

验收：普通用户无法通过 UI 或篡改 URL/请求读取或修改他人资源；API Key 的组/渠道选择始终受后端
Policy 校验，格式由目标推导，前端不能提交策略范围外的目标。

### Phase 3：管理员控制面

- [ ] 逐项实现 users、API Key policy、models、catalog import/sync、channel groups、channels、
  model rules、proxies、config templates、全局日志、审计和手动 reload。
- [ ] 统一复用分页表格处理可能增长的渠道、日志、模型、规则、代理、模板和密钥列表；上游模型按
  provider 分组，并在模型选择器中保持相同分组。
- [ ] 对复杂 JSON 配置实施 schema 校验、可读预览和危险操作确认；不把未验证 JSON 直接提交。
- [ ] 为所有 `PUT` 编辑页实现 ETag 重新加载/冲突恢复体验。

验收：管理员所有写操作仍走现有串行化控制面事务、审计和即时快照发布；前端不绕过 Console API 或直接访问
PostgreSQL。

### Phase 4：质量门禁与运维交付

- [ ] 添加 Rust 静态 UI 集成测试、前端 MSW 组件测试和连接真实 Console/数据库的 Playwright 测试。
- [ ] 在 CI 中验证前端 lockfile、类型检查、lint、测试、构建，以及 `pnpm build` 后的嵌入式 release
  构建。
- [ ] 审查 CSP、Cookie、Origin/CSRF、API Key 显示与复制、一次性邀请 token、错误脱敏、依赖许可证和前端供应链。
- [ ] 更新 `README*`、`config.example.toml`、运维指南和发布说明，明确 API-only 与 UI-enabled 二进制
  的构建/部署方式。

验收：前后端质量门禁均通过；不会因添加 UI 改变任何数据面转发语义。若触及转发路径，仍按项目规则运行
真实上游 smoke test。

## 10. 明确不在本计划内的事项

- 不提供 Chat Playground、面向最终用户的对话界面、第三方嵌入式 Widget 或将用户 prompt 存入浏览器。
- 不将 Console API 改成 GraphQL、BFF 或服务端渲染应用。
- 不在浏览器直接连接 PostgreSQL、上游供应商或读取控制面密钥。
- 不把前端状态当作授权来源，也不以 UI 隐藏替代后端角色校验。
- 不为方便本地开发而降低生产 `Secure` Cookie、CSP 或 API 权限要求。

## 11. 参考实现文档

- [Vite production build](https://vite.dev/guide/build)
- [Vite static deployment](https://vite.dev/guide/static-deploy)
- [shadcn/ui Vite installation](https://ui.shadcn.com/docs/installation/vite)
- [React Router declarative routing](https://reactrouter.com/start/declarative/routing)
- [TanStack Query for React](https://tanstack.com/query/latest/docs/framework/react/installation)
- [rust-embed `RustEmbed`](https://docs.rs/rust-embed/latest/rust_embed/trait.RustEmbed.html)
