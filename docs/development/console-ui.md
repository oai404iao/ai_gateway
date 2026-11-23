# Console Web UI 架构

> 状态：当前。

Console 是 React + TypeScript SPA。Vite 在开发时代理 Console API；生产只需要编入静态资源的 Rust 进程。安装依赖、启动、构建和前端测试命令见[前端 README](../../web/console/README.md)。

## 路由与交付

`src/main.rs` 只在 Console listener 上合并 API 与 UI。`/console/v1/*` 始终优先，未知 API 返回 JSON 404；非 API 的 `GET` / `HEAD` 导航才可能得到 SPA fallback。公共数据面既不提供 Console API，也不提供 UI。

| 静态响应 | 缓存 |
| --- | --- |
| `/`、导航 fallback 及非指纹根资源 | `no-cache`，便于新部署替换入口 |
| `/assets/` 的指纹资源 | `public, max-age=31536000, immutable` |

[`console_ui.rs`](../../src/http/console_ui.rs) 同时设置 CSP、`nosniff`、Referrer Policy 和防嵌入策略。发布时先构建 `web/console/dist`，再用 `embedded-console-ui` 编译并在 TOML 开启 `console.ui_enabled`。未编译 feature 时不能启用该配置；debug 构建从磁盘读取 dist，release 才将其嵌入二进制。

## 页面、会话与缓存

| 位置（相对 `web/console/`） | 职责 |
| --- | --- |
| `src/app/router.tsx`、`src/app/layouts/` | 路由、角色守卫及工作区导航；页面查找目录与导航共用同一份资源表 |
| `src/api/` | 从 [OpenAPI](../openapi/console-v1.yaml) 生成的类型，typed client、会话与刷新 |
| `src/app/providers.tsx` | 按挂载及登录主体创建 `QueryClient`，不在账号之间复用服务端数据 |
| `src/features/` | 页面、表单、数据查询及 mutation |
| `src/components/ui/`、`src/components/shared/` | shadcn/Base UI 原语与复用的业务组件；视觉约定见 [design.md](../../design.md) |

Access token 仅在内存中，refresh token 由 `HttpOnly; Secure; SameSite=Lax` Cookie 传递。并发 401 共用一次刷新；只在刷新后主体与权限范围一致时重试原请求一次。切换用户、角色或强制改密状态会切换 Query Cache，同一主体轮换 access token 不会清空当前视图。前端守卫只控制界面，实际授权取决于[后端主体与资源归属检查](console-auth.md)。

主题与语言是可持久化的非敏感偏好，服务端状态归 TanStack Query，表单草稿不进 URL、Query Cache 或浏览器存储。临时秘密只在当前交互显示。管理员的个人报表和全局报表使用不同 API 和缓存范围。

## 表单与路由编辑

- 使用 React Hook Form 和 Zod。Base UI 自定义 trigger 用 `render`；受 `watch` / `setValue` 控制的 `Select` 必须有 `defaultValues` 或注册字段，否则 `reset()` 后可能缺失验证值。
- GET 保存资源 `ETag`，更新发送 `If-Match`。`409` 不能静默覆盖；保存失败保留草稿，离开页面或切换有草稿的分类先确认。列表来源、键盘返回和窄屏布局见[交互规范](console-interaction-standard.md)。
- 模型编辑器按操作筛选能力候选。单规则编辑在缺少 profile 时先创建 profile，再提交规则，失败重试复用已有 profile。联合上架及单模型批量配置则先预览，再一次性提交核对的候选；版本冲突或结果未知时锁定提交，重载后重新核对。
- 组授权与直接渠道授权是不同选择：选齐组内每个渠道也不等于选择整组。[上游实体](upstream-identity-capabilities.md)说明权限边界。
- 价格以十进制字符串往返；倍率计算由 `src/lib/decimal.ts` 使用 `BigInt` 并按 12 位小数舍入。保存价格时应保留模型元数据、长上下文档位与请求倍率，不把缺失的 `source_payload` 写为空值。

## 验证边界

改 API 时修改 OpenAPI，运行 `pnpm --dir web/console generate:api` 并检查 `generate:api:check`；不手改生成类型。Vitest + MSW 验证组件状态，Playwright 的模拟 API 验证导航、键盘与浏览器行为。这两层不能证明真实数据库授权、Cookie 轮换或扣费；跨进程验收见[系统 E2E](system-e2e.md)。

更改嵌入路径时，从仓库根目录验证实际 Rust 路由和资产响应：

```bash
pnpm --dir web/console build
cargo clippy --locked --all-targets --features embedded-console-ui
cargo test --locked --features embedded-console-ui --lib console_ui
```
