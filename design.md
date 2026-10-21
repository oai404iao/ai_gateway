---
name: ai-gateway-console-design
description: Console 管理界面的表单、密集列表、主从编辑工作区及其明暗主题呈现规范。
---

# Console 界面设计规范

> 状态：当前。以现有 Nova 组件和 CSS 变量为基线；可调整尺寸与拖拽布局为后续选型，尚未实现。

## 1. 范围与优先级

- [MUST] 本规范只约束 Console 的视觉与交互呈现，不定义业务字段、接口或权限。
- [MUST] 冲突时依次保护用户要求、可访问性、可用性、项目组件约定和装饰效果。
- [SHOULD] 保留现有页面结构；不要为统一技术库改变信息顺序或增加交互。

## 2. 表达与读者

- [SHOULD] 面向需要连续阅读表格和表单的管理用户，使用紧凑控件、清晰标题和稳定对齐。
- [SHOULD] 用中性背景承载内容；只用语义颜色强调状态、错误和主要操作。
- [SHOULD] 文案使用直接的动作与结果描述，不在界面展示实现机制或设计推理。

## 3. 页面结构与构图

- [SHOULD] 管理页复用 `ConsoleLayout`：侧栏导航、顶部工具区、`max-w-6xl` 内容区。
  内容区使用 `min-w-0`，外边距使用 `p-4 md:p-6`。
- [SHOULD] 登录类短表单复用 `AuthLayout`：垂直居中，表单容器 `w-full max-w-sm`。
- [SHOULD] 密集列表先显示标题与操作，再显示筛选和表格；表格只在自身容器内横向滚动。
- [SHOULD] 主从编辑工作区在宽屏并列显示选择区和编辑区；窄屏保留现有列表/详情切换，
  不将完整列表堆在编辑器上方，也不把两栏压缩到无法操作。
- [MUST] 当前页面不新增拖拽或缩放把手；普通布局继续使用现有 flex/grid。

## 4. 视觉规则

### 字体与间距

- [MUST] 使用 `src/index.css` 的 Geist 字体入口和 Nova 控件，不为单个页面引入另一套字体。
- [SHOULD] 页面标题复用 `PageHeader` 的 `text-2xl font-semibold tracking-tight`；
  嵌入式标题使用该组件的 `embedded` 变体。
- [SHOULD] 正文、辅助说明和紧凑元数据分别复用组件的 `text-sm`、
  `text-sm text-muted-foreground`、`text-xs`，不缩小主要信息以适应列宽。
- [SHOULD] 表单与操作组使用既有 `gap-*` 和换行布局；不同页面不另设一套间距比例。

### 颜色与表面

- [MUST] 统一使用 shadcn `base-nova` 和集中定义的 CSS 变量。
  新的语义颜色同时定义明暗主题，不在业务组件硬编码另一套调色板。
- [MUST] 背景、文本、边框、焦点和状态使用 `background`、`foreground`、`border`、
  `ring`、`destructive`、`success`、`warning`、`info` 等现有 token。
- [SHOULD] 卡片、弹层、按钮的圆角、边框与阴影由 `components/ui/` 的变体负责；
  页面只调整组合关系，不覆盖控件的基础视觉层级。
- [MUST] 主题切换覆盖所有界面表面，包括 toast；不得给通知单独建立主题来源。

### 图标与数据

- [MUST] 操作、状态和导航图标统一来自 `lucide-react`。按钮图标使用
  `data-icon="inline-start"` 或 `data-icon="inline-end"`，尺寸由按钮负责。
- [SHOULD] 保留项目自有 `GatewayMark` 品牌图形；品牌资产不是操作图标，不以 Lucide 替换商标。
- [SHOULD] 图表沿用 `ChartContainer`、`--chart-*` 和语义颜色；图表标注保留单位。
- [MUST] 控件保留可见焦点、可访问名称以及 disabled/error 状态；不只依赖颜色表达失败。

### 动画

- [MUST] 项目编写的加载、展开及后续编排动画使用 `motion/react`，不再使用
  `tw-animate-css` 或另一个动画库。
- [SHOULD] 复用 `AppProviders` 的 Motion 配置：默认短过渡，不给页面添加装饰性入场动画。
- [MUST] 响应 `prefers-reduced-motion`：Spinner 停止旋转，Skeleton 停止脉动，
  内容和加载语义保持可见；变换动画不阻碍操作。
- [SHOULD] 保留 Nova/Base UI 自带的控件、弹层状态过渡以及第三方组件内部动画；
  减弱动态效果时通过全局 CSS 关闭这些过渡，不为此重写其焦点和卸载生命周期。

## 5. 可用原语与扩展边界

| 角色 | 实现名称 | 源码位置（相对 `web/console/`） | 使用条件 | 状态 |
| --- | --- | --- | --- | --- |
| 语义颜色与圆角 | `--background`、`--foreground`、`--radius` 等 | `src/index.css` | 所有页面 | 已实现 |
| 页面标题 | `PageHeader`、`embedded` | `src/components/shared/page-header.tsx` | 页面与内嵌区标题 | 已实现 |
| 页面容器 | `ConsoleLayout`、`AuthLayout` | `src/app/layouts/` | 管理区、短表单 | 已实现 |
| 操作与状态 | `Button`、`Badge`、`Alert` | `src/components/ui/` | 使用实际导出的变体 | 已实现 |
| 加载反馈 | `Spinner`、`Skeleton` | `src/components/ui/` | 正在加载的区域 | 已实现 |
| 数据展示 | `Table`、`ChartContainer`、`JsonViewer` | `src/components/ui/`、`src/components/shared/` | 表格、图表、折叠 JSON | 已实现 |
| 可调尺寸面板 | `react-resizable-panels` | 尚无封装或调用 | 后续有明确缩放需求时 | 未实现 |
| 拖拽布局 | `dnd-kit` | 尚无封装或调用 | 后续有明确拖拽需求时 | 未实现 |

- [MUST] 页面复用上表和 `components/ui/` 已导出的组件，不凭名称猜测不存在的原语。
- [SHOULD] 页面组合可使用 Tailwind 工具类，无额外 CSS 类名前缀限制；
  仅调整布局和间距，不通过覆盖内部选择器改变原语语义。
- [MUST] 后续出现重复的 resizable + dnd 组合时，在项目自己的
  `src/components/layout/` 封装并复用；只暴露真实页面需要的属性。
  本次不创建占位组件、布局持久化或通用布局配置系统。

## 6. 文案与数字格式

- [SHOULD] 标题说明对象，按钮说明动作，错误信息说明用户可以采取的下一步。
- [MUST] 数量、金额、比例、时长与时间保留对应单位及必要精度，不改变数据含义。
- [SHOULD] 标识符和原始 JSON 使用等宽字体；普通说明不伪装成代码标签。
- [SHOULD] 只在会影响理解或决策时添加说明，放在相关内容旁；
  不默认追加方法论、脚注或页面底部总结，不重复标签已经表达的内容。

## 7. 避免的模式

- [SHOULD] 不把所有管理页面改成居中 hero 和卡片网格。
- [SHOULD] 不为普通元数据添加胶囊标签，不嵌套无意义的卡片。
- [SHOULD] 不添加装饰性图标底板或随机彩色区块。
- [MUST] 不通过隐藏页面横向溢出掩盖布局问题，不缩小正文来容纳宽表格。

## 8. 实现与集成

- [MUST] 主题值与字体入口集中在 [`web/console/src/index.css`](web/console/src/index.css)。
  组件 base/style/icon 来源为 [`components.json`](web/console/components.json)。
- [SHOULD] 主题选择复用 `src/app/theme.tsx` 的 `useTheme`；视觉组件来自
  `src/components/ui/` 和 `src/components/shared/`。
- [SHOULD] 状态归属、依赖版本和验证流程查阅
  [Console 开发指南](docs/development/console-ui.md)，不在视觉规范重复应用架构。

## 术语

| 名称 | 含义 |
| --- | --- |
| Nova | 本项目采用的 shadcn `base-nova` 控件视觉基线 |
| CSS token | `src/index.css` 中可在明暗主题间替换的语义变量 |
| Motion | 项目动画使用的 `motion/react` 入口 |
| layout 组合 | 后续需要时封装的面板缩放与拖拽组合，不是业务状态容器 |
