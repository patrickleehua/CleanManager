# macOS 真机验证指南

更新：2026-10-17。面向在自己的 Mac 上帮忙验证的人。托管 CI 的 macOS runner 只能证明“能编译、单元与夹具测试通过”，不是参考设备（见 [验证计划](../verification-plan.md)）；真机结果才能推进 MVP 第 5、6 组与对标套件的 macOS 任务。

## 现在 macOS 上有什么

| 能力 | macOS 状态 | 能验证什么 |
| --- | --- | --- |
| 扫描 `mac.posix` / `mac.bulk`、硬拒绝路径、凭据存储 | 已实现 | 测试套件中的 macOS 夹具测试；CLI 扫描临时夹具；完全磁盘访问（FDA）开启前后的差异 |
| 索引、规则分析、计划、授权（平台无关部分） | 已实现 | 测试套件；CLI `entries`、`analyze` |
| 移入废纸篓 / 隔离 / 删除、恢复 | 未实现（9.5） | 目前应报告 `unsupported` |
| 清理目录、构建产物、安装包、洞察、卸载、更新、启动项、维护、状态、常驻工具、特权 helper | 未实现 | CLI 应报告 `unsupported`，不应崩溃或误报可用 |
| 桌面应用 | 未在 macOS 上构建过 | `--desktop` 构建前端与 Tauri 宿主测试 |

## 一键收集

需要 Xcode 命令行工具（`xcode-select --install`）、[rustup](https://rustup.rs)，以及构建桌面端时用的 Node.js 22+。

```bash
git clone https://github.com/patrickleehua/CleanManager.git && cd CleanManager
tools/mac_verify.sh              # 完整测试 + clippy + CLI 冒烟（约 10–20 分钟）
tools/mac_verify.sh --desktop    # 另外构建并测试桌面端
tools/mac_verify.sh --quick --scan-dir ~/Downloads   # 只做 CLI 冒烟，并只读扫描一个你选的目录
```

脚本不会修改你的数据：测试只写 `$TMPDIR`，CLI 冒烟使用结果目录里的临时存储和它自己创建的夹具，结束时删除夹具。结果在 `target/mac-verify/<时间>/`（已被 git 忽略），包括：

- `summary.md`：系统版本、芯片、提交号与每一步的通过/失败；
- 每一步的完整日志（`*.log`），以及打包好的 `target/mac-verify/<时间>.tgz`。

请把 `summary.md` 的内容贴到 [GitHub Issue](https://github.com/patrickleehua/CleanManager/issues)，并附上 `.tgz`。`--scan-dir` 的结果包含该目录下的文件名，介意的话不要附上 `cli-scan-user-dir.log`。

## 需要手动做的一项：完全磁盘访问前后（MVP 5.6）

1. 先在“系统设置 › 隐私与安全性 › 完全磁盘访问权限”中**关闭**终端（Terminal/iTerm），重开终端后运行 `tools/mac_verify.sh --quick --scan-dir ~/Library`。
2. 再**打开**终端的完全磁盘访问，重开终端后运行同一条命令。
3. 两次结果目录都附上。期望：没有权限时受保护子树记为 `permission_required` 的跳过范围，而不是空目录；有权限后这些范围被读到。不要把有 FDA 的结果当成“全访问保证”。

## 结果怎样用

- 失败的步骤我会复现并修复，修复后请重新运行同一条命令确认。
- 通过的真机结果会登记到 `docs/evidence/`（记录设备、系统版本、提交号与限制），对应任务才会勾选；CI 通过不会自动勾选任何真机任务。
