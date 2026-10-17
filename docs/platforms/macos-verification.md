# macOS 真机验证指南

更新：2026-10-17。面向在自己的 Mac 上帮忙验证的人。托管 CI 的 macOS runner 只能证明“能编译、单元与夹具测试通过”，不是参考设备（见 [验证计划](../verification-plan.md)）；真机结果才能推进 MVP 第 5、6 组与对标套件的 macOS 任务。

## 现在 macOS 上有什么

| 能力 | macOS 状态 | 能验证什么 |
| --- | --- | --- |
| 扫描 `mac.posix` / `mac.bulk`、硬拒绝路径、凭据存储 | 已实现 | 测试套件中的 macOS 夹具测试；CLI 扫描临时夹具（夹具里有一个故意锁住的目录，结果应为 `partial`、退出码 5）；完全磁盘访问（FDA）开启前后的差异 |
| 索引、规则分析、计划、授权（平台无关部分） | 已实现 | 测试套件；CLI `entries`、`analyze` |
| 移入废纸篓 / 隔离 / 删除、从隔离区恢复与清除 | 已实现（`unverified`） | 测试套件；CLI `capabilities` 应报告 `unverified` 而不是 `unsupported` |
| 手动清理（选择文件移入废纸篓）、废纸篓列表 | 已实现（`unverified`，实验） | CLI `capabilities`；整个文件夹本版不移动 |
| 清理目录（规则第 1 批 33 条，均为实验、只复核不预选） | 已实现（`unverified`，实验） | CLI `clean list` |
| 构建产物、安装包清理 | 已实现（`unverified`，实验） | CLI `purge roots list`、`installer locations` |
| 空间洞察（本地快照数、虚拟机与容器磁盘、iPhone 备份、交换与可清除空间估计、旧下载） | 已实现（`unverified`，实验，只读） | CLI `insights overview` |
| 已安装应用列表 | 已实现（`unverified`，实验，只读） | CLI `apps list` |
| 系统状态与进程、启动项列表、更新检测、维护任务（含性能诊断）、规则第 2 批（26 条） | 已实现（`unverified`，实验；第 4 批已合入 main）。启动项只读，启停为 `unsupported`；需要管理员的维护任务一律 `skipped`（`elevation_unavailable`）；温度与风扇为不可用并说明原因 | CLI `status --count 1`、`process list`、`startup list`、`apps updates`、`optimize list`、`optimize diagnose --window 2`；网络卷 `.DS_Store` 设置可用 `optimize restore` 改回 |
| 卸载、常驻工具 | 未实现（第 5、6 批） | CLI 应报告 `unsupported`，不应崩溃或误报可用 |
| 特权 helper、风扇与电池控制、Touch ID | 本次不做（需要签名或真机） | 应一直报告 `unsupported` |
| 桌面应用 | 未在 macOS 上构建过 | `--desktop` 构建前端与 Tauri 宿主测试 |

“已实现（`unverified`）”的意思是代码与测试都在，但只在托管 runner 上跑过；真机结果登记前不会改成 `verified`。

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

## 已知的真机现象：隐私弹窗会让命令停住

下载、桌面以及其他应用的容器（例如 Docker 的 `~/Library/Containers/com.docker.docker`）受 macOS 隐私保护。第一次读取时系统可能弹出“想要访问……”对话框，回答之前进程会一直等待，没有超时也没有提示。2026-10-17 在 Mac17,14 / macOS 27.0.1 上，第一次运行 `insights overview` 用了 155 秒，第二次只用了 0.4 秒，结果相同。看到某一步特别慢时，先找一下有没有没回答的对话框，并在提交结果时说明你点了允许还是拒绝。

## 结果怎样用

- 失败的步骤我会复现并修复，修复后请重新运行同一条命令确认。
- 通过的真机结果会登记到 `docs/evidence/`（记录设备、系统版本、提交号与限制），对应任务才会勾选；CI 通过不会自动勾选任何真机任务。
