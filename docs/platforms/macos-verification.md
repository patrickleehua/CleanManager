# macOS 真机验证指南

更新：2026-10-18。面向在自己的 Mac 上帮忙验证的人。托管 CI 的 macOS runner 只能证明“能编译、单元与夹具测试通过”，不是参考设备（见 [验证计划](../verification-plan.md)）；真机结果才能推进 MVP 第 5、6 组与对标套件的 macOS 任务。

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
| 应用卸载（App 移入废纸篓、Homebrew、AI 命令行工具、孤立残留） | 已实现（`unverified`，实验）。App Store 与 root 拥有的 App 报告需要管理员并给出访达/启动台指引；受保护的 Apple 应用被拒绝 | CLI `apps uninstall list`、`apps uninstall orphans`；只用你愿意卸载的测试 App 实际执行 |
| 常驻工具（菜单栏图标、保持亮屏、清洁屏幕、摄像头/麦克风使用提示、电量提醒） | 已实现（`unverified`，实验）；开机自启需要签名包，本版为 `unsupported` | 桌面应用“状态 › 常驻工具”；`pmset -g assertions` 可看到保持亮屏的断言 |
| 特权 helper、风扇与电池控制、Touch ID | 本次不做（需要签名或真机） | 应一直报告 `unsupported` |
| 桌面应用 | 宿主已接入全部 macOS 后端，CI 在 macOS 上跑宿主 clippy 与测试；尚未在真 Mac 上启动过 | `--desktop` 构建前端与 Tauri 宿主测试；再用 `cd apps/desktop && npx tauri dev` 实际打开，检查各页面的 macOS 文案 |

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

## 已知的真机现象：隐私弹窗会让读取停住（已处理）

下载、桌面、文稿以及其他应用的数据受 macOS 隐私保护（TCC）。负责的应用（终端、Claude、桌面版 CleanManager）还没有做过选择时，第一次打开这些位置会弹出“想要访问……”对话框，**回答之前那次 `open(2)` 一直卡在内核里**：没有超时，也不会返回 `EPERM`。2026-10-17 在 Mac17,14 / macOS 27.0.1 上，第一次运行 `insights overview` 用了 155 秒，第二次只用了 0.4 秒，结果相同。

2026-10-18 在同一台 Mac 上用一个专用测试包（bundle ID `com.cleanmanager.tccprobe`，放在 `$TMPDIR`，`tccutil reset` 只针对这个 ID）复现并确认了机制：

- `lstat(~/Downloads)` 立刻返回；`opendir(~/Downloads)` 内部的 `open(O_DIRECTORY)` 卡住。`sample` 显示主线程停在 `__open`。tccd 日志在同一时刻记录了 `kTCCServiceSystemPolicyDownloadsFolder` 的 `display_prompt`，请求方是 sandboxd，等到 `CFUserNotification response` 之后调用才返回。
- 由 launchd 在 Background 会话里启动的同一程序也一样：对话框照样出现在登录用户的屏幕上，调用照样卡住，不会因为没有界面就得到 `EPERM`。
- 卡住的只有发起调用的线程，其他线程照常运行，进程也能在它卡住时正常退出。对话框在进程退出后仍然留在屏幕上，回答结果照常记录下来。
- 已经允许后不再等待，读取立刻返回。这次没有实测“不允许”的情形，按 Apple 的说明应立刻得到 `EPERM`（报告为 `permission_required`）。不允许弹窗的服务（日志里的 `kTCCServiceDeveloperTool`；完全磁盘访问也从不弹窗）由 tccd 直接拒绝，不会卡住。
- Docker Desktop 不是沙盒应用，读取 `~/Library/Containers/com.docker.docker` 时没有触发任何 TCC 检查。

现在的处理：空间洞察第一次打开这些位置（下载文件夹、iPhone 备份、其他应用容器内的磁盘映像）时，会放到单独的线程上执行，最多等 5 秒。超时后该位置报告为 `permission_required (permission_cause=privacy_prompt_pending)`，大小未知，不按 0 或空计算，命令随后继续执行。`--debug` 会在等待超过 250 毫秒时写一行 `privacy: … macOS may be showing a privacy prompt`，超时时再写一行。桌面端会显示“正在等待 macOS 授权”，并提示回答对话框后点“重新测量”。对话框回答之前，再次请求同一位置会立刻报告等待中，不会再开新线程。

验证时如果看到某张卡片或某一项显示等待授权，请回答对话框后重新运行同一条命令，并在提交结果时说明你点了允许还是拒绝。其他命令（磁盘分析扫描、清理目录解析等）打开这些位置时还没有这层保护；遇到哪一步特别慢，先看看有没有没回答的对话框。

## 结果怎样用

- 失败的步骤我会复现并修复，修复后请重新运行同一条命令确认。
- 通过的真机结果会登记到 `docs/evidence/`（记录设备、系统版本、提交号与限制），对应任务才会勾选；CI 通过不会自动勾选任何真机任务。
