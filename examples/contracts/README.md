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

这些是独立契约场景，不是连续执行轨迹。`plan.json` 显示尚待授权；操作示例假设另一场景中授权已经通过，授权凭据本身不在可修改的计划 JSON 里。示例规则仍是 experimental；候选文案中的假定应用身份不能证明真实规则已验证。

生成器只重写项目内的 Schema/样例：`python tools/generate_contracts.py`。验证器运行方法见 [项目文档](../../docs/README.md)。
