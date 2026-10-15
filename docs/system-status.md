# 系统状态、进程与空间洞察卡片（Windows）

状态：变更 `add-mole-parity-suite` 任务 12.3（进程列表）、12.4（可选持续高占用提醒、状态流过期标记）与 7.3（概览洞察卡片、旧下载审阅）的 Windows 部分已实现；macOS 返回 `unsupported`。核心 `cm_core::{process, alerts, status, insights}`，服务 `cm_app::{processes, insights}`，平台 `windows/processes.rs`、`windows/status.rs`、`windows/insights.rs`。命令形状以 [CLI 协议](cli-protocol.md) 为准。

## 进程列表（STS-04）

- 事实只来自本机只读查询：ToolHelp 快照（PID、父进程、名称），`PROCESS_QUERY_LIMITED_INFORMATION` 句柄读取路径、创建时间、CPU 时间、I/O 传输计数、私有内存与“系统关键进程”标记；CPU/I/O 速率取 1 秒窗口内两次读数之差。读不到的值（受保护进程、其他用户的进程）为 `null` 并附说明，从不写 0。
- `disk_bytes_per_sec` 是进程读写传输速率（文件、设备与网络 I/O 计数之和），任务管理器的“磁盘”列使用 ETW，二者可能不同；每进程网络速率需要 ETW 内核跟踪（管理员），本版本不采样，恒为 `null` 并说明原因。
- 签名：先验证内嵌 Authenticode，再查系统安全目录（in-box 二进制多为目录签名）；无界面、不做吊销或联网检索。结果为 `signed`、`catalog_signed`、`unsigned` 或 `unknown`。
- 用途说明来自本地知识表，只有名称、所在目录与签名者三者都一致时才显示；名称像系统进程但位置或签名不符时标为未识别并说明不符之处。未识别进程 `identified: false`，带 `ai_diagnosis` 入口字段（仅提示，列表本身不发送任何数据）。
- 排序 `--by cpu|memory|disk|network|name`（未知值排在已知值之后），`--search` 在名称、PID、路径、签名者、所属应用与用途中查找；置顶的进程名（`process pin`，保存在存储目录的 `process-pins.json`）始终排在最前且不被搜索过滤。

## 结束进程

状态页除结束进程外不提供任何处置。`process end PID` 只生成 `process` 计划（恰好一个 `process_end` 动作，绑定 PID、创建时间、可执行路径与其 SHA-256，不可被策略匹配）；`plan review` 在终端逐次确认，`plan apply` 执行前用同一个句柄重新核对创建时间、路径、摘要与保护判定后才调用 `TerminateProcess`，PID 被复用或程序被替换时跳过（`precondition_changed`）。

以下进程不提供结束入口，计划创建与执行时各拒绝一次（`policy_denied` / `protected`）：System、Registry、Memory Compression、smss、csrss、wininit、winlogon、services、lsass、LsaIso、fontdrvhost、dwm、Windows 标记为关键的进程、CleanManager 自身，以及终端安全软件（按进程名与签名者识别）。

```text
cleanmanager process list --by memory --limit 20 --json   # process_list
cleanmanager process show 4242 --json                     # 单个进程的本地事实
cleanmanager process pin code.exe                         # process_pins
cleanmanager process end 4242 --json                      # plan（process），需审阅
cleanmanager plan review pp_demo                          # 终端内逐次确认
cleanmanager plan apply pp_demo --json                    # 重新核对身份后结束
```

## 持续高 CPU 提醒（STS-05）

默认关闭；`status alerts --state on [--cpu-threshold 80] [--window-seconds 120] [--cooldown-seconds 1800]` 开启（阈值 1–100%，窗口 10–3600 秒，冷却 60–86400 秒），设置保存在 `status-alerts.json`。提醒只在状态流（`status --watch`，或后续的常驻组件）运行时生效：每个采样周期读取每进程 CPU（占整机比例），同一程序（按可执行路径，否则按名称）在窗口内每次采样都不低于阈值才提醒一次（`status_alert` 记录，`link` 指向该进程详情），冷却期内即使换了 PID 也不重复；采样低于阈值、未知或间隔超过 60 秒都会重新计时。关闭时不做任何进程采样。

## 状态流（STS-06）

`status --watch [--interval N] [--count M]` 每行一个 `status_snapshot`。采样在独立线程进行；每个输出时刻若有新完成的样本则输出它，否则（采样器变慢）再次输出上一个样本并标 `stale: true`，`sampled_at` 保持原采样时间，绝不改写为当前时间。不可得指标为 `null` 并附原因。

## 概览洞察卡片与旧下载（DIN-03/04）

- `insights overview` 输出 `insight_overview`：旧下载、虚拟机与容器、还原点与快照、休眠/分页文件、组件存储、旧 Windows 安装、更新分发缓存、开发工具缓存、AI 模型库、设备备份等卡片。每张卡片给出已知大小，或 `logical_bytes: null` 加 `size_note`（部分成员无法测量时不把部分和当总数），以及成员中最早的观测时间；`count` 对快照给出还原点数量，大小不可得时只显示数量。`link` 指向磁盘分析位置（`analyze` + 路径）、旧下载审阅或已知对象列表。桌面端调用 `cm_app::insights::overview`（输入已收集时用纯函数 `overview_wire`）。
- `insights downloads [--older-than-days 90] [--limit 50]` 输出 `old_downloads_review`：下载目录（含 3 层子目录，不跟随链接，最多 20000 项）中修改时间早于阈值的文件，按大小排序，`selected` 恒为 `false`；处理只能走手选移入回收站（`trash`）或安装包审阅（`installer scan`），从不是默认候选。
