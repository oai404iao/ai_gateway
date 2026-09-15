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

`/codex-sharing` 是本人金额视图；`/admin/codex-sharing` 和其详情页负责不依赖用户组的固定
席位配置、ETag 保存和逐席位用量。它们位于 `src/features/codex-sharing/`，不与只读 Codex
官方配额页混用；本人金额接口不返回其他成员或凭证信息。API Key 编辑器把席位可选的逻辑凭证
和 API Key Policy 普通目标分成响应式双栏，前者只暴露路由所需 UUID/格式，不暴露 provider
identity 或 Token。规则见 [Codex 拼车](codex-sharing.md)。
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
- `/admin/model-setup` 是计价模型、Channel 和协议路由三个专用管理面的轻量入口，不复制这些
  页面中的编辑器或状态推导。普通 Channel 和计价模型的复制仍由各自列表/详情页负责；复制 URL
  只能携带资源 UUID 和经过校验的 `/admin/*` 返回路径，不能序列化凭据或完整资源。

### 模型配置导航与层级编辑

模型配置侧边栏入口默认打开 `/admin/routing/model-rules`。`configuration-navigation.tsx` 只
提供计价模型、Channel 和模型规则之间的统一导航；各资源使用自己的表格和详情页，不再维护重复的
目录/检查器状态或 `?mode=table` 分支。

- 模型规则列表每个计价模型只显示一行，并在同一行列出其协议及状态。创建顶层规则时，只能从已
  启用且尚未绑定顶层规则的计价模型中选择。
- `/admin/routing/model-rules/:id` 显示不可编辑的客户端计价身份，以及 Chat Completions、
  Responses 和 Images 协议入口。缺失协议由此创建为停用的空 `draft`。
- `/admin/routing/model-rules/:id/protocols/:protocolId` 是实际路由编辑器。协议格式不可编辑；
  每个 tier 直接编辑可重复 Channel、但渠道/模型组合唯一的显式候选和独立权重。Channel Group
  下拉框只把当前尚未选择的成员批量展开为候选，保存结果不保留组引用，后续成员变化不会隐式
  改写规则。上游模型仍只能从各 Channel 的 `available_models` 选择，不提供自由文本输入。
- 渠道组、渠道、计价模型和协议规则编辑器继续使用统一双栏详情和 sticky 操作栏；价格编辑器
  保留专用计算布局。每次保存仍只提交一个资源，没有跨资源原子保存。
- 生产使用 data router 的 `useBlocker` 保护 PUSH/REPLACE/浏览器 POP；表单草稿不持久化。
  Declarative `AppRouter` 留给组件测试，fallback 保护应用链接和返回操作；真正的 POP
  保护由详情页测试覆盖。页面刷新/关闭由 `beforeunload` 保护。

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
