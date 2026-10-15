# 合成契约样例

这些文件用于设计、前后端联调准备和 Schema 校验。`ExampleApp`、文件身份、哈希、时间、容量与授权状态均是构造值，不是本机扫描结果，也不能交给真实执行器直接执行。

| 文件 | 表达的场景 |
| --- | --- |
| [capabilities.json](capabilities.json) | 平台能力可用、需要权限的不同状态 |
| [scan-partial.json](scan-partial.json) | 部分子树拒绝访问，已知数据与缺口并存 |
| [candidate.json](candidate.json) | Windows 路径原始 UTF-16 编码和候选理由 |
| [candidate-macos.json](candidate-macos.json) | macOS 路径原始字节编码 |
| [plan.json](plan.json) | 固定对象集合、15 分钟有效期、规范化摘要 |
| [needs-approval.json](needs-approval.json) | 未获授权时的结构化拒绝 |
| [operation-trash.json](operation-trash.json) | 假定已通过独立授权后的同卷 Trash 操作，空闲变化为 0 |
| [progress.json](progress.json) | 总量未知的进度，避免虚假百分比 |
| [plan-v2-cleanup.json](plan-v2-cleanup.json) | plan/v2 清理计划：回收站、隔离、删除与所属工具适配，唯一可被策略匹配的种类 |
| [plan-v2-manual-selection.json](plan-v2-manual-selection.json) | 手选对象原位移入回收站 |
| [plan-v2-manual-trash-purge.json](plan-v2-manual-trash-purge.json) | 仅对回收站内对象永久删除 |
| [plan-v2-uninstall.json](plan-v2-uninstall.json) | 退出应用（询问强制）、卸载器适配、残留、注册表先备份、启动项 |
| [plan-v2-maintenance.json](plan-v2-maintenance.json) | 维护任务与可逆设置，含需管理员任务 |
| [plan-v2-startup.json](plan-v2-startup.json) | 停用一个计划任务启动项 |
| [plan-v2-recovery.json](plan-v2-recovery.json) | 注册表恢复（已存在则跳过）、启动项与设置恢复 |
| [request-clean-preview.json](request-clean-preview.json) | 非交互 dry-run 请求，只含 default_selected |
| [preview-clean.json](preview-clean.json) | 三档候选、占用跳过、受保护对象与一致的汇总 |
| [request-apps-uninstall.json](request-apps-uninstall.json) | 非交互卸载计划请求 |
| [needs-approval-uninstall.json](needs-approval-uninstall.json) | 卸载计划缺授权 |
| [unsupported-capability.json](unsupported-capability.json) | 平台不支持的能力 |
| [protection-unavailable.json](protection-unavailable.json) | 保护名单不可读，写操作停止 |
| [app-list.json](app-list.json) | 应用清单，含受保护的安全代理 |
| [startup-list.json](startup-list.json) | 启动项清单，含失效项 |
| [maintenance-catalog.json](maintenance-catalog.json) | 维护任务目录与适用性 |
| [maintenance-report.json](maintenance-report.json) | 拒绝提权时的逐项结果 |
| [status-snapshot.json](status-snapshot.json) | 不可得指标为 null、公开健康分权重 |
| [protection-list.json](protection-list.json) | 三层保护，仅用户条目可移除 |
| [insight-list.json](insight-list.json) | 休眠文件与旧下载洞察 |
| [doctor-report.json](doctor-report.json) | 脱敏诊断报告 |
| [scan-status-running.json](scan-status-running.json) | 运行中扫描的状态（不含终态枚举） |
| [scan-cancel-receipt.json](scan-cancel-receipt.json) | 取消回执，只表示请求已接收 |
| [scan-interrupted.json](scan-interrupted.json) | 宿主进程退出后的 interrupted 终态，保留已知结果 |
| [entries-page.json](entries-page.json) | 绑定 revision 的目录分页，未知大小为 null |
| [candidates-page.json](candidates-page.json) | 候选分页 |
| [analysis-result.json](analysis-result.json) | candidate_set 结果，部分扫描下的待判断证据 |
| [recovery-plan-restore.json](recovery-plan-restore.json) | recovery/v1 恢复计划，目标冲突时跳过 |
| [authorization-policy.json](authorization-policy.json) | 有限策略授权及额度 |
| [authorization-required.json](authorization-required.json) | 尚无授权 |
| [operation-status-running.json](operation-status-running.json) | 运行中的操作 |
| [operation-cancel-receipt.json](operation-cancel-receipt.json) | 操作取消回执，不撤销已完成动作 |
| [operation-reconciliation.json](operation-reconciliation.json) | 存在未知结果，需核对（退出 8） |
| [quarantine-list.json](quarantine-list.json) | 隔离清单，含目标冲突项 |
| [purge-roots.json](purge-roots.json) | 默认与用户添加的项目扫描根 |
| [plan-v2-update.json](plan-v2-update.json) | 应用更新：询问退出后经包管理器升级 |
| [release-info.json](release-info.json) | 包管理器安装的副本提示使用对应升级方式 |
| [self-update-result.json](self-update-result.json) | 校验和不符时中止更新 |
| [self-remove-preview.json](self-remove-preview.json) | 自卸载预览：默认保留配置、保护名单、历史与隔离区 |
| [completion-powershell.json](completion-powershell.json) | 仅输出、不写入配置的补全脚本 |

这些是独立契约场景，不是连续执行轨迹。`plan.json` 显示尚待授权；操作示例假设另一场景中授权已经通过，授权凭据本身不在可修改的计划 JSON 里。示例规则仍是 experimental（含 `rules/examples/v2` 的 rule 2.0 样例），实测登记样例标记 synthetic；候选文案中的假定应用身份不能证明真实规则已验证。

生成器只重写项目内的 Schema/样例：`python tools/generate_contracts.py`。验证器运行方法见 [项目文档](../../docs/README.md)。
