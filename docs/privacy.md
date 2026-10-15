# 隐私说明（发行版）

更新：2026-10-15。适用于 CleanManager CLI 与桌面应用的当前构建。CleanManager 没有账户、遥测、崩溃上报、广告或后台联网；它不向本项目的任何服务上传数据。

## 保存在本机的数据

除非另行说明，以下内容保存在每用户存储目录（Windows：`%LOCALAPPDATA%\CleanManager\store`，可用 `CM_STORE_DIR` 改变），位于当前用户的配置文件目录中（默认权限只允许该用户与管理员访问），不会被自动发送到任何地方：

| 数据 | 文件 | 内容 | 保留 |
| --- | --- | --- | --- |
| 扫描索引 | `index.db` | 你扫描的目录树：路径、名称、大小、时间、文件标识；不含文件内容 | 可重建的缓存，卸载时默认删除 |
| 操作与恢复记录 | `audit.db`、`migrations.log`、`audit.db.pre-v*.bak` | 计划、授权、逐项结果、隔离清单、注册表/启动项备份、迁移日志 | 卸载时默认保留；待核对记录不会被普通清理移除 |
| 隔离区 | `quarantine\`，或同卷根目录的 `.cleanmanager-quarantine\` | 你选择隔离的文件本身 | 只有你执行恢复或清空计划才变化；卸载从不删除 |
| 设置 | `protection.json`、`purge-settings.json`、`app-updates.json`、`status-alerts.json`、`process-pins.json`、`diag-providers.json` | 保护名单、项目根、检查开关、提醒阈值、置顶进程、AI 提供方设置（不含密钥） | 卸载时默认保留 |
| 上次预览 | `purge-last-run.json`、`installer-last-run.json` | 最近一次构建产物/安装包预览 | 缓存 |
| AI 诊断密钥 | Windows 凭据管理器 `CleanManager:diag:*` | 你自行提供的 API 密钥 | 只在你配置 AI 诊断时存在 |

CleanManager 还可能在你确认后写入：shell 配置文件中的补全加载块（`completion install`）、用户 PATH 中的安装目录（`self install --add-to-path`）、你指定的诊断报告文件（`doctor --export`，默认脱敏）。`self uninstall` 移除前两者。

日志与记录只包含任务 ID、类型化状态与必要的对象位置，不记录文件内容、环境变量、令牌或凭据。`doctor` 报告默认把用户目录、用户名与计算机名替换为 `~`、`<user>`、`<host>`，导出前先预览。

## 只在你操作时离开本机的数据

| 功能 | 何时 | 发送什么、发给谁 |
| --- | --- | --- |
| AI 诊断（可选，默认未配置） | 你在预览中确认发送后 | 预览中逐字显示的脱敏证据包与问题，发给**你配置的**提供方端点（或本机 CLI）；不含文件内容或整盘目录树，摘要变化即需重新确认。详见 [AI 诊断](ai-diagnostics.md) |
| CLI 自更新检查/下载 | 你运行 `self update [--check]` 时 | 向更新通道（构建时配置或 `--manifest` 指定的 HTTPS 地址）请求签名清单与文件；对方可见 IP 地址、时间与 `User-Agent: CleanManager/<版本> (self-update)`。不发送扫描结果、文件列表或设置。包管理器安装与未配置通道的构建不发起任何请求 |
| 应用更新检查与更新 | 你运行 `apps updates` / 批准更新计划时 | 由 winget/Scoop 等包管理器访问它们自己配置的源；CleanManager 不上传应用清单。`apps updates config --checks off` 后不发起任何检查 |

没有周期性检查：不运行上述命令就没有任何网络请求。外部 Agent 通过 CLI 读取的 JSON 结果之后如何处理取决于该 Agent 的运行环境，CleanManager 无法控制。

## 删除你的数据

`cleanmanager self uninstall` 默认删除程序与缓存、保留配置/历史/隔离区；隔离区为空且无待核对操作时可加 `--remove-user-data` 同时删除已识别的配置、保护名单、历史与 AI 密钥。也可以在恢复或清空隔离区之后手动删除存储目录。安装与卸载细节见 [安装、更新与卸载](install.md)。
