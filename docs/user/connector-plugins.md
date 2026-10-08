# 安装连接器插件

> 状态：当前。原生插件由管理员独立安装；不提供在线安装、自动升级或热卸载。

## 适用范围

`general` 内置，无需插件文件。Codex OAuth 的协议实现由独立
[ai-gateway-connectors](https://github.com/oai404iao/ai-gateway-connectors)
仓库发布，网关镜像与发行包不包含该库，也不会回退到旧内置实现。
升级已有 Codex 部署前，必须先准备与网关 ABI 兼容的插件制品。

插件与网关处于同一进程，能够读取进程内存或导致进程崩溃。
只安装已经审核的可信制品；SHA-256 校验不能把恶意原生代码变为安全代码。
第一版只支持 Linux，必须选择匹配 CPU 架构和 libc 基线的制品。

## 安装

1. 从插件仓库的发行页获取对应归档和校验和，验证发布来源和归档摘要。
2. 检查归档内的 manifest、平台、ABI、插件版本和许可证材料。
3. 将库安装到不受非授权用户写入的绝对路径。目录不能通过符号链接访问，
   文件及其父目录必须满足加载器的所有权和权限检查。
4. 使用**动态库文件本身**的 SHA-256 填入 TOML，不是归档摘要。

例如，以 root 管理安装目录：

```bash
sudo install -d -m 0755 /opt/ai-gateway/plugins/codex
sudo install -m 0444 libai_gateway_connector_codex.so \
  /opt/ai-gateway/plugins/codex/libai_gateway_connector_codex.so
sha256sum /opt/ai-gateway/plugins/codex/libai_gateway_connector_codex.so
```

网关配置：

```toml
[[plugins]]
id = "codex"
path = "/opt/ai-gateway/plugins/codex/libai_gateway_connector_codex.so"
sha256 = "<替换为上一步的64位十六进制摘要>"
```

加载器先校验制品，再加载冻结的同一份字节；缺失库、摘要不符、ID 不符、
不兼容 ABI、非法 manifest 或不安全文件权限会阻止启动。重复 ID 及覆盖 `general` 被拒绝。
仍被未删除接入或凭证引用的插件不能直接卸载，停用草稿也保留这一依赖。
历史日志、金额事实和已删除资源不需要重新执行插件。

## Docker Compose

默认镜像不包含插件。创建本地 Compose override，只读挂载安装目录：

```yaml
services:
  gateway:
    volumes:
      - /opt/ai-gateway/plugins:/opt/ai-gateway/plugins:ro
```

使用实际 Compose 服务名，并在容器 TOML 中填写容器内路径。
插件文件应为 root 所有且可读、不可写；不要把运行时 UID 10001 无权读取的
宿主私有目录直接作为挂载源。不得跳过原有 entrypoint、特权降低或 secret 准备流程。
选择与网关运行镜像兼容的发行制品；宿主能加载的库不一定能在容器内加载。

## 配置与检查

管理员可通过 `GET /console/v1/system/connectors` 查看当前进程实际加载的连接器版本、
ABI 及操作上限。Console 接入与静态凭证表单使用该列表，不支持的操作不能配置为可用能力。
新增插件不会自动创建接入、凭证、渠道、模型或路由。

Codex 的凭证导入、OAuth、Token/额度维护继续使用现有 Console 入口；
网络、事务、权限、日志和拼车金额仍由网关管理。
外部连接器与静态凭证必须显式匹配，`codex_oauth` 不能被绑定到其他连接器。

## 升级与恢复

插件二进制只在启动时加载。升级步骤：

1. 阅读插件状态 schema 和 ABI 兼容性说明，保存原制品及原配置。
2. 按后端指南完成一致备份；SQLite 仍需数据库与 spool 成对备份。
3. 停止网关，安装已校验的新库并更新 TOML 摘要。
4. 启动网关，核对连接器列表，并验证所用操作。

不能在线覆盖库后期待在途请求切换到新版本。重启会清空 WS 连接局部缓存；
客户端必须处理 `previous_response_not_found`，不能要求网关将 continuation 改投其他连接。
不兼容的凭证状态或数据库升级必须使用配套恢复方案，不能只回滚 `.so`。

## 相关文档

- [设计与 ABI 边界](../development/connector-plugins.md)
- [上游凭证管理](upstream-credentials.md)
- [生产部署](production-deployment.md)
- [SQLite 备份与恢复](sqlite.md)
