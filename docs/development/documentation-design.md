# Pi 文档研究与本项目组织设计

> 状态：当前。记录文档组织和编写决策，不定义网关运行时行为。

本项目借鉴 Pi 的任务导航、渐进展开和明确边界，不复制其产品分类或功能承诺。
本页解释参考了哪些材料、现有文档的问题，以及这些写法如何落到网关手册。
日常编写规则见[文档规范](../documentation-standard.md)。

## 研究范围与来源

研究日期为 **2026-10-16**，对象是本地安装的 `@earendil-works/pi-coding-agent` **1.2.0** 文档快照，
版本及仓库位置来自该包的 `package.json`。它指向
[earendil-works/pi](https://github.com/earendil-works/pi) 的 `packages/coding-agent`。
下列上游链接用于定位同名页面，默认分支可能变化；研究结论针对该安装包，不声称代表上游最新版本。

| 材料 | 主要观察 |
| --- | --- |
| [README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md) | 产品说明、最短开始路径、开发与许可证；详细能力链接到专题 |
| [index.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/index.md) | 按开始使用、自定义、自动化、查参考、安全运行分流 |
| [docs.json](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/docs.json) | 导航按 Get Started / Guides / Reference 组织，另有旧路径重定向 |
| [quickstart.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/quickstart.md) | 安装、启动、选模型、做任务，再给继续使用和自定义选择表 |
| [configuration.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/configuration.md) | 先解释配置层次，再用路径/职责表区分来源和优先级 |
| [how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md) | 以循环、上下文、会话、接口、资源、权限解释系统，而不是罗列历史 PR |
| [cli.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/cli.md) | 参数按用途分组，调用示例与不兼容条件相邻 |
| [security.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/security.md) | 区分信任决定与系统隔离，直说权限仍然允许哪些行为 |

专题改写还参考了模型/提供商、会话格式和 CLI 集成等页面的表格、示例与契约表达。
这些是写作模式观察，不是采用 Pi SDK、扩展系统或配置机制的决定。

## 本项目原有问题

| 原有阅读成本 | 调整方向 |
| --- | --- |
| 两份 README 分别约 424/543 行，安装、运行边界、配置表与开发说明混在一起 | 缩短为一致的产品概览与入口；完整流程移到快速上手和专题 |
| 总索引不断追加阶段进展，读者需要先理解项目演进 | 首页只回答“我现在要做什么”，各分类索引提供完整专题地图 |
| `operations.md` 约 973 行，首次调用与恢复细节交织 | 保留稳定入口，按数据面、路由、Console、计费和运维任务分层 |
| 设计记录叠加旧目标与新实现，部分“当前”段落已经过时 | 依据源码、测试、migrations 和契约修正当前说明；历史依据独立标识 |
| 操作说明重复定义 API 或提供商策略 | 机器契约保留唯一来源，Markdown 解释使用方式和边界 |
| “通过验收”与测试入口、生产保证混用 | 区分本次检查、历史测量、待执行验证和长期不变量 |

行数来自改写前快照，只说明审计起点，不作为文档质量指标。
删减重复内容不能以丢失财务原子性、迁移兼容、安全或恢复限制为代价。

## 采用的阅读路径

```text
README.md / README.zh-CN.md      是什么、支持范围、从哪里开始
  └─ docs/README.md             按任务选择入口
       ├─ user/quickstart.md    走通本地启动到第一条请求
       ├─ user/                 部署、配置、使用、运维与安全
       ├─ development/          修改、验证、设计和发布
       ├─ reference/            外部语义及网关兼容边界
       ├─ openapi/              机器可读 Console 契约
       └─ archive/              仅用于追溯
```

### 从最小可验证结果开始

Pi 的 quickstart 不要求先读完参考再完成任务。本项目同样先给可以检查的结果，
但必须分清 `/health` 存活与代理就绪：数据库为空时前者可以成功，后者仍需管理员、
凭证、能力、路由和客户端 Key。不能为了“简短”省略这段配置关系。

### 按问题命名入口

首页使用“启动、配置与部署”“使用 API 与管理上游”“理解实现与参与开发”“查阅契约与历史”。
分类仍保留用户、维护者和外部参考的责任边界，专题页先说明用途，再展开步骤或概念。
长文用少量关键锚点帮助跳转，避免目录比问题本身更复杂。

### 用结构表达区别

借鉴 Pi 的配置路径表和能力选择表，本项目用表格区分进程 TOML 与数据库运行时设置、
用户操作与管理员操作、原生 ABI 与命令协议、支持的 API 操作与不支持范围。
示例只展示必要字段，完整字段仍回到 OpenAPI、配置模板或请求策略 JSON。

### 明确保护与不保护的边界

Pi 的安全页没有把“信任项目”等同于沙箱。本项目也不能把启用原生插件等同于进程隔离，
把进程本地准入当作集群硬限制，把软额度当作预付预留，或把单节点 Compose 当作 HA/备份方案。
说明限制应贴近相关操作，不只在页尾放一段通用警告。

## 不照搬的部分

- 不复制 Pi 的自定义、自动化和 SDK 产品分类；网关的核心任务是转发、配置、授权与恢复。
- 不把所有专题搬到单层目录。现有分类承载了读者和来源边界，稳定路径也便于旧链接与发行包继续使用。
- 不引入文档站框架、`docs.json` 导航生成器或新构建依赖。当前 Markdown 索引已能承载阅读路径。
- 不按 Pi 的 API 示例改动网关接口；不把机器可读 OpenAPI/JSON 转成手工 Markdown 契约。
- 不翻译全部页面、重写历史档案或许可证声明。保留原语言，双语 README 保持关键内容一致。
- 不为了统一模板，把简短参考和复杂恢复手册压成相同行数或相同标题。

## 验收方法

结构检查执行 `git diff --check` 和 `python3 scripts/check-docs.py`，另核对新增入口、
重命名标题和相对链接。事实审查重点检查曾经混入旧设计的插件归属、上游拓扑、SQLite 状态、
路由操作与独立计量来源；不能只以 Markdown 校验通过判断改写完成。

命令与当前脚本、清单和配置模板交叉核对。文档中的历史测试结果保留其适用范围，
本次未运行的系统、上游和负载测试不报告为通过。后续维护仍执行[变更同步矩阵](../documentation-standard.md#变更同步矩阵)。
