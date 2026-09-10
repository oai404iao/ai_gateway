# Console Web UI 架构与开发指南

> 状态：当前。实现以 `web/console/`、`src/http/console_ui.rs`、Console OpenAPI
> 和测试为准。原始分阶段计划保存在
> [Console UI 实施计划归档](../archive/console-ui-implementation-plan.md)。

系统设置分类定义在 `web/console/src/features/admin/system/settings-sections.ts`，侧栏的
可展开子菜单与 `/admin/system/:section` 页面共用此定义。旧 `/admin/system` 地址跳转
到基础设置；每个分类切换时重建表单，保留完整 API 配置及 ETag，只挂载当前分类控件。

## 1. 运行边界

Console UI 是管理 `ai-gateway` 的 React 单页应用，不是聊天产品或第三方 Widget。

- 源码位于 `web/console/`。
- 生产构建输出到 `web/console/dist`，可通过 `embedded-console-ui` Cargo feature 编入 Rust
  单二进制。
- UI 只由独立 Console listener 提供；公共 `/v1/*` listener 绝不挂载 UI、静态资源或
  `/console/v1/*` API。
- Rust 服务是唯一生产运行时；Node.js、pnpm 和 Vite 只用于开发、测试和构建。
- 不使用 SSR、Next.js、常驻 Node 服务或运行时可配置的静态目录。

## 2. 当前技术栈

| 领域 | 当前选择 |
| --- | --- |
| UI | React 19 + TypeScript（strict）+ Vite |
| 样式/组件 | Tailwind CSS v4 + shadcn/ui `base-nova`，primitives 使用 Base UI |
| 路由 | React Router |
| 服务端状态 | TanStack Query |
| 表单 | React Hook Form + Zod |
| 测试 | Vitest + Testing Library + MSW；Playwright Chromium |
| Lint | oxlint；不使用 ESLint |
| API 类型 | 从 `docs/openapi/console-v1.yaml` 生成 |

`web/console/components.json` 和 `web/console/package.json` 是前端 base/style 与依赖的直接来源。
shadcn 组件以源码形式保存在 `src/components/ui/`，不是运行时黑盒组件包。

## 3. 路由规则

```text
HTTPS reverse proxy
  -> Console listener
       ├── /console/v1/*   JWT Console API
       ├── /assets/*       fingerprinted Vite assets
       └── GET/HEAD SPA navigation fallback
```

API router 在 SPA fallback 之前合并。未匹配的 `/console/v1/*` 返回 JSON 404，绝不返回
`index.html`。SPA fallback 只响应 `GET`/`HEAD`；公共 listener 的路由测试必须继续证明
Console API/UI 不可达。

## 4. 目录与契约

`/codex-sharing` 是本人金额视图；`/admin/codex-sharing` 和其详情页负责固定席位配置、
ETag 保存和逐席位用量。它们位于 `src/features/codex-sharing/`，不与只读 Codex 官方配额页
混用，也不向本人接口返回其他成员或凭证信息。规则见 [Codex 拼车](codex-sharing.md)。
Codex 渠道组编辑器提供“仅拼车使用”开关；普通 Connector 不显示该控件。
同池配对组模式同步，但不联动格式启用或 Key 授权，成员普通渠道仍按原权限使用。

```text
web/console/
  src/api/           typed client、session store、generated OpenAPI types
  src/app/           providers、router、layout、theme、i18n
  src/features/      auth、profile、usage、API keys、admin control plane
  src/components/ui/ shadcn/Base UI primitive wrappers
  src/components/shared/
  src/test/          Vitest setup、MSW、deterministic fixtures
  e2e/               Playwright browser smoke tests
```

Console API 形状的唯一来源是 `docs/openapi/console-v1.yaml`：

```text
docs/openapi/console-v1.yaml
  -> pnpm --dir web/console generate:api
  -> web/console/src/api/generated/console-v1.d.ts
  -> web/console/src/api/types.ts re-export shim
```

禁止手改 generated declaration。变更契约时同时提交规范和生成结果，并运行
`generate:api:check`。

## 5. 会话与安全

- access token 只保存在浏览器内存，绝不写入 `localStorage`、`sessionStorage`、URL 或持久化
  Query Cache。
- refresh token 只存在于 `HttpOnly; Secure; SameSite=Lax` Cookie。生产 Console listener 应置于
  HTTPS 反向代理之后。
- API client 对并发 401 使用 single-flight refresh，并只重试原请求一次。
- 前端 role-aware 导航只是 UX；后端 JWT ownership/admin 校验始终是授权边界。
- API Key 默认打码；一次性邀请/注册 secret 不进入 Query Cache 或浏览器持久化。
- 可更新资源从 GET 响应保存 ETag，并在 PUT/DELETE 发送 `If-Match`；`409` 应提示重新加载，不能
  静默覆盖。
- `src/http/console_ui.rs` 为 UI 响应添加 CSP、`nosniff`、Referrer Policy 和防嵌入策略。

## 6. Base UI 与表单约定

- 项目已经从 Radix 迁移到 Base UI。自定义 trigger 使用 Base UI 的 `render`，不要重新引入
  `asChild` 或 `@radix-ui/*`。
- 对 shadcn wrapper 的修改必须检查 Base UI 的 popup/portal anatomy、状态属性和键盘语义；不要假设
  Radix DOM 或 `data-state` 行为仍存在。
- React Hook Form 中所有由 `form.watch` / `form.setValue` 驱动的 `Select` 字段都必须出现在
  `useForm({ defaultValues })` 中，否则 `reset()` 后该字段不会参与 validation，提交可能静默失败。
- 复用 `src/components/ui/` 与 `src/components/shared/` 中已有组件；业务组合放在对应
  `src/features/`。
- TypeScript 启用了 `verbatimModuleSyntax`、`noUnusedLocals`、`noUnusedParameters` 和
  `erasableSyntaxOnly`：使用 `import type`，不使用 TypeScript enum，不保留未使用变量。
- `QueryClient` 必须按 `AppProviders` mount 创建，不能改成模块级 singleton。
- 模型价格使用十进制字符串作为表单和 API 状态。`/admin/models/:id/pricing` 的倍率计算通过
  `src/lib/decimal.ts` 使用 `BigInt` 做十进制定点乘法并按 12 位小数舍入，不能用 JavaScript
  `number` 的二进制浮点结果直接写回价格字段。该子页保存完整 `ModelInput` 时必须保留模型元数据、
  长上下文档位和请求倍率，并省略不在普通详情响应中的 `source_payload`。桌面布局使用左侧价格/
  星期时段编辑区和右侧 sticky 计算、摘要、保存工具栏，窄屏按 DOM 顺序降级为单栏。星期 Toggle
  Group 必须保持至少一个 UTC 开始星期，旧响应缺少 `weekdays` 时在表单层归一化为全周。
- `/admin/model-setup` 是渠道组、普通供应商 Channel、模型价格和模型规则之上的前端编排页，
  复用既有 list/detail/mutation hooks，不引入另一套 Console API 或原子批量写入语义。复制入口的
  URL 只能携带资源 UUID 和经过校验的 `/admin/*` 返回路径，不能序列化凭据或完整资源。普通
  Channel 复制默认清空 `upstream_api_key`，且目标渠道组被限制为来源 API 格式；Codex
  provider-managed Channel 不进入复制候选；
  模型复制清空全局唯一的 `source_model_id`，并明确提示普通详情响应不包含的 `source_payload`
  不会复制。

### 配置工作台

模型配置侧边栏入口默认打开 `/admin/routing/model-rules`。三个列表路由复用
`model-setup/configuration-workbench.tsx`，分别是客户端路由、渠道供给和模型价格视角，
而不是按数据库资源排列的多个折叠列表：

- 桌面左侧是可搜索、按格式或供应商筛选的分页目录，右侧是请求路径和关联资源面板；
  窄屏在目录和选中面板之间切换。目录每页 24 项，单组渠道每页 8 项。
- `q`、`facet`、`state`、`page`、`selected` 保存在 URL；资源间关联跳转用 UUID 定位。
  失效的显式选择显示不可用提示，不能静默展示另一个资源。
- `configuration-graph.ts` 只推导资源引用关系。路由状态和可路由渠道计数使用 API 返回值，
  不把模型能力列表或本地开关推断为实时健康。`all` 和 `selected` 目标必须区别处理。
- 同一 Codex pool 在目录中合并，但 Responses/Images group 的开关、能力和路由分别展示；
  凭据仍进入专用管理页面，普通 Channel 的复制和批量操作不能编辑托管凭据。
- `?mode=table` 是显式的表格/批量工具；保留批量渠道编辑、恢复、组禁用和规则快速添加。
  `/admin/model-setup` 保留为创建流程向导，不再充当日常配置总览。
- 渠道组、渠道、模型、规则编辑器使用统一双栏详情和 sticky 操作栏；价格编辑器保留专用
  计算布局。通过工作台进入的编辑器保存成功后返回经过校验的 `returnTo`，保持原筛选和选择；
  每次保存仍只提交一个既有资源，没有跨资源原子保存。
- 生产使用 data router 的 `useBlocker` 保护 PUSH/REPLACE/浏览器 POP；表单草稿不持久化。
  Declarative `AppRouter` 留给组件测试，fallback 保护应用链接和返回操作；真正的 POP
  保护由 `e2e/configuration-workbench.spec.ts` 验证。页面刷新/关闭由 `beforeunload` 保护。

## 7. 开发与生产运行

开发模式：

```bash
# Terminal 1: Console API on 127.0.0.1:3001
cargo run

# Terminal 2: HTTPS Vite server and /console/v1 proxy
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console dev
```

打开 `https://console.localhost:5173`。Vite 是开发工具；此模式下 `[console].ui_enabled` 无意义。

嵌入式生产构建：

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build
cargo build --release --features embedded-console-ui
```

然后设置 `[console].ui_enabled = true`。如果未编译 feature，启动会拒绝该配置。Debug feature
构建时 `rust-embed` 从磁盘读取 `web/console/dist`，因此 dist 仍须存在；release 构建会嵌入资产。

## 8. 静态资源与缓存

| 路径 | 行为 | Cache-Control |
| --- | --- | --- |
| `/` 与 SPA fallback | `index.html` | `no-cache` |
| `/assets/<fingerprinted>` | 嵌入式资产 | `public, max-age=31536000, immutable` |
| 根目录非指纹资产 | 嵌入式资产 | `no-cache` |
| `/console/v1/*` | Console API | `no-store` |

## 9. 测试与验证

```bash
pnpm --dir web/console generate:api:check
pnpm --dir web/console typecheck
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build
pnpm --dir web/console e2e:install # first run only
pnpm --dir web/console e2e
```

- Vitest 只收集 `src/**/*.{test,spec}.{ts,tsx}`；`e2e/` 明确排除。
- MSW handler 使用相对路径，fixtures 保持确定性。
- Playwright 使用 `vite.e2e.config.ts` 在 `127.0.0.1:5174` 提供 HTTP SPA，并在浏览器网络层 mock
  `/console/v1/*`，不需要 Rust 或 PostgreSQL。
- 当前 oxlint 的 5 个 shadcn Fast Refresh warning 是已知允许项；不能新增 warning 或 error。
- 嵌入式 serving 路径变化时，先构建 dist，再运行：

  ```bash
  cargo clippy --locked --all-targets --features embedded-console-ui
  cargo test --locked --features embedded-console-ui --lib console_ui
  ```

后端静态 UI 测试位于 `src/http/console_ui.rs` 和相关 router 单元测试中，不存在单独的
`tests/console_ui_integration.rs`。

## 10. 相关来源

- 前端命令与测试说明：[`web/console/README.md`](../../web/console/README.md)
- Console API：`docs/openapi/console-v1.yaml`
- Rust 静态路由：`src/http/console_ui.rs`
- 启动与 router 合并：`src/main.rs`
- Console JWT 设计：[Console 认证与授权](console-auth.md)
