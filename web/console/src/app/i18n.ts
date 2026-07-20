import { createContext, useContext } from "react";

export type ConsoleLocale = "en-US" | "zh-CN";

// This stores only a display-language preference. Console access tokens remain
// memory-only and refresh credentials remain in their HttpOnly cookie.
const STORAGE_KEY = "ai-gateway-console.locale";

const zhCN: Record<string, string> = {
  "Personal": "个人",
  "Administration": "管理",
  "Routing": "路由",
  "Operations": "运维",
  "Profile": "个人资料",
  "Sessions": "会话",
  "API Keys": "API 密钥",
  "Request Logs": "请求日志",
  "Users": "用户",
  "API Key Policies": "API 密钥策略",
  "Upstream Models": "上游模型",
  "Catalog": "模型目录",
  "Channel Groups": "渠道组",
  "Channels": "渠道",
  "Model Rules": "模型规则",
  "Proxies": "代理",
  "Templates": "模板",
  "Audit Logs": "审计日志",
  "System": "系统",
  "Console": "控制台",
  "Light": "浅色",
  "Dark": "深色",
  "System default": "跟随系统",
  "Toggle theme": "切换主题",
  "Language": "语言",
  "English": "English",
  "简体中文": "简体中文",
  "Sign out": "退出登录",
  "Signed out": "已退出登录",
  "Administrator": "管理员",
  "User": "用户",
  "Chat Completions": "聊天补全",
  "Responses": "响应",
  "Weighted random": "加权随机",
  "Weighted round-robin": "加权轮询",
  "Bearer token": "Bearer 令牌",
  "Custom header": "自定义请求头",
  "Succeeded": "成功",
  "Failed": "失败",
  "Rejected": "已拒绝",
  "Cancelled": "已取消",
  "Active": "启用",
  "Enabled": "已启用",
  "Disabled": "已禁用",
  "Revoked": "已撤销",
  "Suspended": "已暂停",
  "Invited": "待激活",
  "Yes": "是",
  "No": "否",
  "never": "永不过期",
  "just now": "刚刚",
  "{minutes}m ago": "{minutes} 分钟前",
  "{hours}h ago": "{hours} 小时前",
  "{days}d ago": "{days} 天前",
  "Sign in": "登录",
  "Use your Console account to continue.": "使用您的控制台账户继续。",
  "Email": "邮箱",
  "Password": "密码",
  "Received an invitation?": "收到邀请？",
  "Activate it": "激活账户",
  "Activate invitation": "激活邀请",
  "Set your password to activate the Console account you were invited to.":
    "设置密码以激活受邀的控制台账户。",
  "Invitation token": "邀请令牌",
  "New password": "新密码",
  "Activate account": "激活账户",
  "Already have an account?": "已有账户？",
  "Signed in": "已登录",
  "Sign in failed": "登录失败",
  "Account activated": "账户已激活",
  "Activation failed": "激活失败",
  "Enter a valid email.": "请输入有效的邮箱地址。",
  "Password must be at least 12 characters.": "密码至少需要 12 个字符。",
  "Invitation token is required.": "邀请令牌不能为空。",
  "Your Console identity and security settings.": "您的控制台身份和安全设置。",
  "Your own proxied requests, usage, and settlement state.": "您的代理请求、用量和结算状态。",
  "All proxied requests across every user and API key.": "所有用户和 API 密钥的代理请求。",
  "Account": "账户",
  "Read-only account facts.": "只读账户信息。",
  "Role": "角色",
  "Status": "状态",
  "Balance": "余额",
  "Created": "创建时间",
  "Updated": "更新时间",
  "Display name": "显示名称",
  "Shown to administrators and in audit records.": "管理员和审计记录中显示的名称。",
  "Save display name": "保存显示名称",
  "Change password": "修改密码",
  "Changing your password immediately signs out every active session.":
    "修改密码会立即退出所有活跃会话。",
  "Current password": "当前密码",
  "Confirm new password": "确认新密码",
  "Profile updated": "个人资料已更新",
  "Update failed": "更新失败",
  "Password changed. All sessions were signed out.": "密码已修改，所有会话均已退出。",
  "Password change failed": "密码修改失败",
  "Display name is required.": "显示名称不能为空。",
  "At least 12 characters.": "至少需要 12 个字符。",
  "Passwords do not match.": "两次密码输入不一致。",
  "Console users, roles, and balances. New users join by invitation.":
    "控制台用户、角色和余额；新用户通过邀请加入。",
  "Invite user": "邀请用户",
  "Name": "名称",
  "Invitation issued": "邀请已创建",
  "Invite failed": "邀请失败",
  "Review the highlighted invitation fields.": "请检查标记的邀请字段。",
  "The invitation token is shown once and must be delivered out of band.":
    "邀请令牌只显示一次，需通过安全渠道发送给用户。",
  "Currency": "币种",
  "Currency is required.": "币种不能为空。",
  "Default API key policy": "默认 API 密钥策略",
  "None": "无",
  "Send invitation": "发送邀请",
  "Give this to the new user to activate their account.": "请将此令牌提供给新用户以激活账户。",
  "Account, access, and balance changes take effect immediately.":
    "账户、访问权限和余额变更会立即生效。",
  "Manage a Console user's identity, role, and status.": "管理控制台用户的身份、角色和状态。",
  "Back to users": "返回用户列表",
  "Edit user": "编辑用户",
  "Save user": "保存用户",
  "User updated": "用户已更新",
  "Account updated. Sign in again to continue.": "账户已更新，请重新登录后继续。",
  "This user was changed elsewhere. Reloading.": "此用户已在其他位置修改，正在重新加载。",
  "Review the highlighted account fields.": "请检查标记的账户字段。",
  "Set the current account balance in the selected currency.": "设置所选币种下的当前账户余额。",
  "Enter a valid balance.": "请输入有效的余额。",
  "Name is required.": "名称不能为空。",
  "Proxy URL is required.": "代理 URL 不能为空。",
  "Client model is required.": "客户端模型不能为空。",
  "Pick a channel group.": "请选择渠道组。",
  "Base URL is required.": "基础 URL 不能为空。",
  "Source model id is required.": "上游模型 ID 不能为空。",
  "Pick at least one format.": "请至少选择一种 API 格式。",
  "Upstream model": "上游模型",
  "Upstream model identifiers and their prices. Prices carry an effective timestamp.":
    "上游模型标识及其价格。价格带有生效时间。",
  "New upstream model": "新建上游模型",
  "Back to upstream models": "返回上游模型",
  "Create upstream model": "创建上游模型",
  "Edit upstream model": "编辑上游模型",
  "Save upstream model": "保存上游模型",
  "An upstream model identifier with its billing price.": "具有计费价格的上游模型标识。",
  "Source payload is not valid JSON.": "来源数据不是有效的 JSON。",
  "Upstream model created": "上游模型已创建",
  "Upstream model updated": "上游模型已更新",
  "This upstream model was changed elsewhere. Reloading.": "此上游模型已在其他位置修改，正在重新加载。",
  "Save failed": "保存失败",
  "Map (client model, API format) to one priced upstream model and routing targets.":
    "将客户端模型和 API 格式映射到一个带价格的上游模型及路由目标。",
  "New rule": "新建规则",
  "New model rule": "新建模型规则",
  "Back to rules": "返回规则列表",
  "Create rule": "创建规则",
  "Edit rule": "编辑规则",
  "Save rule": "保存规则",
  "Model rule created": "模型规则已创建",
  "Model rule updated": "模型规则已更新",
  "This rule was changed elsewhere. Reloading.": "此规则已在其他位置修改，正在重新加载。",
  "Routes a client model and API format to one priced upstream model and channels.":
    "将客户端模型和 API 格式路由到一个带价格的上游模型和渠道。",
  "Client model": "客户端模型",
  "API format": "API 格式",
  "Pick an upstream model.": "请选择上游模型。",
  "Pick an upstream model": "选择上游模型",
  "Description": "说明",
  "Channel groups ({count})": "渠道组（{count}）",
  "Channels ({count})": "渠道（{count}）",
  "No groups for this format.": "该格式没有可用渠道组。",
  "No channels for this format.": "该格式没有可用渠道。",
  "priority {priority}": "优先级 {priority}",
  "Preview, import, or explicitly update models.dev prices.":
    "预览、导入或显式更新 models.dev 价格。",
  "Preview": "预览",
  "Fetch the models.dev catalog, optionally filtered by provider ids.":
    "获取 models.dev 目录，可按提供商 ID 筛选。",
  "Provider ids (optional)": "提供商 ID（可选）",
  "Fetch preview": "获取预览",
  "Apply selected ({count})": "应用所选项（{count}）",
  "Preview results": "预览结果",
  "{count} catalog models.": "{count} 个目录模型。",
  "No catalog models": "没有目录模型",
  "No models matched the preview request.": "没有模型匹配此预览请求。",
  "Select rows to import new models or update existing model prices.":
    "选择行以导入新模型或更新已有模型的价格。",
  "{importable} new, {updatable} updatable, {selected} selected.":
    "{importable} 个新模型，{updatable} 个可更新，已选择 {selected} 个。",
  "Preview failed": "预览失败",
  "Imported {imported}, updated {updated} price(s).":
    "已导入 {imported} 个模型，已更新 {updated} 个模型价格。",
  "Apply failed": "应用失败",
  "Model": "模型",
  "Provider": "提供商",
  "Input price": "输入价格",
  "Output price": "输出价格",
  "Source model id": "上游模型 ID",
  "Provider name": "提供商名称",
  "Price unit tokens": "价格单位 Token 数",
  "Input unit price": "输入单价",
  "Cached input unit price": "缓存输入单价",
  "Cache write unit price": "缓存写入单价",
  "Output unit price": "输出单价",
  "Price effective at": "价格生效时间",
  "Source payload (JSON, optional)": "来源数据（JSON，可选）",
  "Prices are per the configured price unit tokens.": "价格按配置的价格单位 Token 数计算。",
  "Effective": "生效时间",
  "Action": "操作",
  "Select": "选择",
  "Targets": "路由目标",
  "{count} groups": "{count} 个渠道组",
  "{count} channels": "{count} 个渠道",
  "import": "导入",
  "price update": "价格更新",
  "already exists": "已存在",
  "Filters": "筛选",
  "Filter by exact model, request outcome, format, time range, and settlement state.":
    "按精确模型、请求结果、格式、时间范围和结算状态筛选。",
  "User ID": "用户 ID",
  "API key ID": "API 密钥 ID",
  "Exact client or upstream model": "精确客户端模型或上游模型",
  "All formats": "所有格式",
  "Outcome": "结果",
  "All outcomes": "所有结果",
  "Billing": "计费",
  "All billing": "所有计费状态",
  "Billed": "已结算",
  "Unbilled": "未结算",
  "From": "开始时间",
  "To": "结束时间",
  "Results": "结果数",
  "Last {count}": "最近 {count} 条",
  "Filter actions": "筛选操作",
  "Apply": "应用",
  "Clear": "清除",
  "Requests": "请求",
  "The gateway never stores prompts or completions.": "网关不会存储提示词或生成内容。",
  "No request logs": "没有请求日志",
  "There are no logged requests matching these filters.": "没有符合这些筛选条件的请求日志。",
  "Started": "开始时间",
  "HTTP": "HTTP",
  "Output tokens": "输出 Token",
  "Cost": "成本",
  "Duration": "耗时",
  "Request log": "请求日志",
  "Loading…": "加载中…",
  "HTTP status": "HTTP 状态",
  "Streamed": "流式",
  "TTFT": "首字节时间",
  "Total duration": "总耗时",
  "Input tokens": "输入 Token",
  "Cached input": "缓存输入",
  "Cache write": "缓存写入",
  "Billed at": "结算时间",
  "Error code": "错误代码",
  "Channel group": "渠道组",
  "Channel": "渠道",
  "Completed": "完成时间",
  "Format": "格式",
  "Group": "渠道组",
  "State": "状态",
  "Weight": "权重",
  "Priority": "优先级",
  "Strategy": "策略",
  "auto-disabled": "自动禁用",
  "Load-balancing pools for one API format with priority tiers.":
    "按 API 格式和优先级划分的负载均衡池。",
  "New group": "新建渠道组",
  "Upstream endpoints inside a channel group with weight, timeouts, and auth.":
    "渠道组内具有权重、超时和认证配置的上游端点。",
  "New channel": "新建渠道",
  "yes": "是",
  "no": "否",
  "Request failed": "请求失败",
  "Confirm": "确认",
  "Cancel": "取消",
  "Back": "返回",
  "Danger zone": "危险操作",
  "These actions are permanent and audited.": "这些操作不可撤销，且会记录审计日志。",
  "Save it now": "立即保存",
  "This value is shown only once. The gateway does not store it in a retrievable form, and the console will not display it again.":
    "此值只显示一次。网关不会以可检索的形式存储它，控制台也不会再次显示。",
  "Copied": "已复制",
  "Copy": "复制",
  "I have saved it": "我已保存",
  "Console rejected the request ({code}).": "控制台拒绝了该请求（{code}）。",
  "An unexpected error occurred.": "发生了意外错误。",
  "Nothing here yet": "这里暂时没有内容",
  "There are no items to show.": "暂无可显示内容。",
  "No records": "没有记录",
  "There are no records to show yet.": "暂无可显示记录。",
  "Click a row to view or edit.": "点击行以查看或编辑。",
};

let activeLocale: ConsoleLocale = "en-US";

export function browserLocale(): ConsoleLocale {
  if (typeof window === "undefined") return "en-US";
  const stored = window.localStorage.getItem(STORAGE_KEY);
  if (stored === "en-US" || stored === "zh-CN") return stored;
  return window.navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

export function currentLocale(): ConsoleLocale {
  return activeLocale;
}

export function setCurrentLocale(locale: ConsoleLocale): void {
  activeLocale = locale;
}

export function translate(
  key: string,
  values: Record<string, string | number> = {},
): string {
  return translateFor(activeLocale, key, values);
}

export function translateFor(
  locale: ConsoleLocale,
  key: string,
  values: Record<string, string | number> = {},
): string {
  const template = locale === "zh-CN" ? (zhCN[key] ?? key) : key;
  return template.replace(/\{(\w+)\}/g, (_, name: string) => String(values[name] ?? `{${name}}`));
}

interface I18nContextValue {
  locale: ConsoleLocale;
  setLocale: (locale: ConsoleLocale) => void;
  t: (key: string, values?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);
export { I18nContext, STORAGE_KEY };

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext);
  if (!context) {
    throw new Error("useI18n must be used within I18nProvider");
  }
  return context;
}
