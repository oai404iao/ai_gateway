---
name: ai-gateway-console-design
description: Console 桌面与移动工作区的导航、概览图表、密集列表及编辑表单的明暗主题呈现规范。
---

# Console 界面设计规范

> 状态：当前。适用于 Console，不适用于项目网站或文档。

## 1. 范围与优先级

- [MUST] 本规范只约束 Console 的视觉与交互呈现，不定义业务字段、接口或权限。
- [MUST] 冲突时依次保护用户要求、可访问性、可用性、项目组件约定和装饰效果。

## 2. 表达与读者

- [SHOULD] 面向反复比较数据与调整配置的读者，让标题、图表和表格共享左边线，数字沿列对齐。
- [SHOULD] 文案使用直接的动作与结果描述，不在界面展示实现机制或设计推理。

## 3. 页面结构与构图

- [MUST] 工作区复用 `ConsoleLayout`。桌面侧栏顶部放品牌与查找，中部导航可独立滚动，底部保留账户与偏好入口。
  进入 `useIsMobile` 窄屏断点时，品牌与账户移到页首，常用入口通常固定在底部，正文末尾为导航与设备安全区留出高度。宽度不超过 24rem 时，常用入口在页首品牌行下方随正文滚动，不覆盖图表时段与操作。
- [SHOULD] 局部导航在同一侧栏展开，不再占一列。窄屏将其移到正文上方，让表格与编辑器使用完整宽度。
- [SHOULD] 概览将时间范围放在标题旁，可切换的指标横列于折线图上方；选中指标有底线，数值列靠右对齐。明细紧接图表，不重复整组指标。
- [SHOULD] 密集列表先显示标题与操作，再放视图、紧凑筛选和表格；优先让首条记录可见，宽表格只在自身容器内横向滚动。
- [SHOULD] 主从编辑工作区在宽屏并列显示选择区和编辑区；窄屏保留现有列表/详情切换，
  不将完整列表堆在编辑器上方，也不把两栏压缩到无法操作。
- [SHOULD] 登录类短表单复用 `AuthLayout`：品牌在页首，窄表单居中，主操作与字段同宽，不附加工作区导航。
- [MUST] 当前页面不新增拖拽或缩放把手；普通布局继续使用现有 flex/grid。

## 4. 视觉规则

### 字体与间距

- [MUST] 使用 `src/index.css` 的 Geist 字体入口和 Nova 控件，不为单个页面引入另一套字体。
- [SHOULD] 页面标题复用 `PageHeader`；嵌入式标题使用 `embedded` 变体。概览可放大主标题与指标值，单位和比较基准仍用次级字号。
- [SHOULD] 正文、辅助说明和紧凑元数据分别复用组件的 `text-sm`、
  `text-sm text-muted-foreground`、`text-xs`，不缩小主要信息以适应列宽。
- [SHOULD] 表单与操作组使用既有 `gap-*` 和换行布局；不同页面不另设一套间距比例。

### 颜色与表面

- [MUST] Console 的基础色取自 [ReUI Shared KPI Panels](https://reui.io/blocks/application/card/card-28) **Base / Nova / Neutral** 默认预览的明、暗主题。`src/index.css` 保存其同名语义变量；数值以该文件为准。其他 Block 的局部示例色不替代全局 `primary`。

| 用途 | 色彩来源与应用 |
| --- | --- |
| 画布、侧栏与控件 | 使用预览的 `background`、`sidebar`、`primary`、`accent`、`ring` 与相应前景色。亮色模式保留浅侧栏；当前页用 `sidebar-accent` 和字重，焦点另用 `sidebar-ring`。 |
| 指标与图表容器 | 按 Shared KPI Panels：Frame 为 `muted` 的 50% 透明度、面板为 `card`、边线为 `border`，值与标签分别用 `foreground` 和 `muted-foreground`。切换指标仅移动 `primary` 底线，不改变面板色。 |
| 快捷入口 | 按 [ReUI Topic Shortcuts](https://reui.io/blocks/application/card/card-41) 的 **靛蓝、橙、天蓝 600** 实色图标底与白色线条；其余入口用中性 `primary`。色块限于图标，不铺满卡片。 |
| 状态与通知 | 按 [ReUI Badge](https://reui.io/docs/badge) 与[语义色定义](https://reui.io/docs/styling)使用紫色 `info`、黄色 `warning`、红色 `destructive`。成功态是非绿色例外，用 ReUI `secondary` 中性底与前景，保留状态文字或勾号。用量变化不是健康状态，只显示有符号的数值和方向。 |
| 多序列统计 | 沿用 [shadcn-admin 的明暗图表色](https://github.com/satnaing/shadcn-admin/blob/e16c87f213a5ba5e45964e9b67c792105ec74d26/src/styles/theme.css)：依次取上游 `chart-1`、`chart-3`、`chart-4`、`chart-5`，跳过含绿色的第二组；第五序列沿用其[单序列图的 primary](https://github.com/satnaing/shadcn-admin/blob/e16c87f213a5ba5e45964e9b67c792105ec74d26/src/features/dashboard/components/overview.tsx)。图例、曲线和明细引用同一序列色，不用状态色拼盘。 |
| 资源表格 | 按 [shadcn-admin Tasks Table](https://github.com/satnaing/shadcn-admin/blob/e16c87f213a5ba5e45964e9b67c792105ec74d26/src/features/tasks/components/tasks-table.tsx)使用整体细边框、无独立色带的表头，悬停和选中分别用 `muted/50` 与 `muted`。分页与筛选在表框外。 |

- [MUST] 以上角色在明暗主题、菜单和 toast 中一致；页面不得另定义品牌色或按指标混色。

### 图标与数据

- [MUST] 操作、状态和导航图标统一来自 `lucide-react`。按钮图标使用
  `data-icon="inline-start"` 或 `data-icon="inline-end"`，尺寸由按钮负责。
- [SHOULD] 保留项目自有 `GatewayMark` 品牌图形；品牌资产不是操作图标，不以 Lucide 替换商标。
- [SHOULD] 图表沿用 `ChartContainer` 与 `--chart-*`；同一序列在图例、图线与明细中使用同色。坐标轴写清单位，选中时段能读到时间与数值，不要求读者仅靠颜色或悬停推算。
- [MUST] 控件保留可见焦点、可访问名称以及 disabled/error 状态；不只依赖颜色表达失败。

### 动画

- [MUST] 项目编写的加载、展开及后续编排动画使用 `motion/react`，不再使用
  `tw-animate-css` 或另一个动画库。
- [SHOULD] 复用 `AppProviders` 的 Motion 配置：默认短过渡，不给页面添加装饰性入场动画。
- [MUST] 响应 `prefers-reduced-motion`：Spinner 停止旋转，Skeleton 停止脉动，
  内容和加载语义保持可见；变换动画不阻碍操作。
- [SHOULD] 保留 Nova/Base UI 自带的控件、弹层状态过渡以及第三方组件内部动画；
  减弱动态效果时通过全局 CSS 关闭这些过渡，不为此重写其焦点和卸载生命周期。

## 5. 可用原语

| 角色 | 实现名称 | 源码位置（相对 `web/console/`） |
| --- | --- | --- |
| 语义颜色与圆角 | `--background`、`--foreground`、`--radius` 等 | `src/index.css` |
| 页面与内嵌区标题 | `PageHeader`、`embedded` 变体 | `src/components/shared/page-header.tsx` |
| 页面容器与断点 | `ConsoleLayout`、`AuthLayout`、`useIsMobile` | `src/app/layouts/`、`src/hooks/use-mobile.ts` |
| 操作与状态 | `Button`、`Badge`、`Alert` | `src/components/ui/` |
| 同组面板 | coss ui `Frame`、`FramePanel` | `src/components/ui/frame.tsx` |
| 加载反馈 | `Spinner`、`Skeleton` | `src/components/ui/` |
| 数据展示 | `Table`、`ChartContainer`、`JsonViewer` | `src/components/ui/`、`src/components/shared/` |

- [MUST] 页面复用上表和 `components/ui/` 已导出的组件，不凭名称猜测不存在的原语。
- [SHOULD] 页面组合可使用 Tailwind 工具类，无额外 CSS 类名前缀限制；
  仅调整布局和间距，不通过覆盖内部选择器改变原语语义。

## 6. 文案与数字格式

- [SHOULD] 标题说明对象，按钮说明动作，错误信息说明用户可以采取的下一步。
- [MUST] 概览、图表（含提示）、列表与详情摘要中的 Token 按十进制千进位：少于 1,000 不加后缀，其余按量级用 `K`、`M`、`B`，最多两位小数，不补零；舍入达到下一档时进位。中英文共用后缀，如 `999`、`1.25K`、`2.3M`、`1B`。请求数不是 Token。
- [MUST] 只读金额按语言分组，最多五位小数，四舍五入后去掉末尾零；保留币种，列头或坐标轴已有单位时可只显示数值。`24.50000` 显示为 `24.5 USD`，真实零为 `0 USD`；绝对值不足 `0.00001` 的非零金额显示为 `<0.00001 USD`（负值为 `>-0.00001 USD`），缺失为 `—`。
- [MUST] 账单核对与定价阈值可补充分组的精确 Token 整数。编辑、复制原值与原始 JSON 不使用展示格式；计算、排序与提交始终使用原始数值。
- [SHOULD] 可比较的数值使用 `tabular-nums`；未知值用缺失标记，与实际的零区分。标识符和原始 JSON 使用等宽字体；普通说明不伪装成代码标签。
- [SHOULD] 资源以名称、设备、模型调用名或其他可辨识内容为主标签；内部 ID 仅用于消歧、复制或排障，放在需要它的详情中。
- [SHOULD] 标题下默认不加副文案；只在范围、单位、权限或操作后果会影响决策时添加说明，放在相关内容旁；
  不默认追加方法论、脚注或页面底部总结，不重复标签已经表达的内容。

## 7. 避免的模式

- [SHOULD] 不把所有管理页面改成居中 hero 和卡片网格。
- [SHOULD] 不为普通元数据添加胶囊标签，不嵌套无意义的卡片。
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
