# 系统状态、进程与空间洞察卡片（Windows / macOS）

状态：变更 `add-mole-parity-suite` 任务 12.3（进程列表）、12.4（可选持续高占用提醒、状态流过期标记）、12.5（显示偏好与网络合计去重）与 7.3（概览洞察卡片、旧下载审阅）的 Windows 部分已实现；macOS 的状态采样、僵尸进程计数、进程列表与结束进程见下文“macOS”一节（只在托管 CI 运行器上测试过，能力为 `unverified`）。核心 `cm_core::{process, alerts, status, insights}`，服务 `cm_app::{processes, insights}`，平台 `windows/processes.rs`、`windows/status.rs`、`windows/insights.rs`、`macos/status.rs`、`macos/processes.rs`。命令形状以 [CLI 协议](cli-protocol.md) 为准。

默认关闭（设计 D15）：系统状态与进程（G3）和空间洞察（G1）尚未通过各自的门禁，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

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

## 网络合计去重（STS-07）

- 数据来源：`GetIfTable2`（IP Helper，只读）给出每个接口的 IANA 类型、隧道类型、`HardwareInterface` 标志与收发字节计数；速率是相邻两次读数之差除以间隔。NDIS 过滤层（`FilterInterface`，与其底层网卡重复计数）和非“已连接”的接口不参与。核心分类在 `cm_core::status::network`，与平台无关。
- 分类规则（按顺序）：类型 24（软件回环）→ `loopback`，不列出、不计入，只在 `loopback_excluded` 计数；类型 131（隧道，如 Teredo、IP-HTTPS）、类型 23（PPP，系统自带 VPN 拨号）或隧道类型非 0 → `tunnel`；类型 53（专有虚拟，如 Wintun 一类的 VPN 驱动）或驱动声明非硬件（Hyper-V/WSL `vEthernet`、虚拟交换机、Wi-Fi Direct 虚拟网卡、TAP）→ `virtual`；硬件且类型明确 → `physical`；其余（类型 1“其他”、未报告硬件标志）→ `unknown`。
- 合计 = `counted`（`physical` 与 `unknown`）之和；`tunnel`/`virtual` 放在 `separate` 中单独显示、从不加入合计（隧道流量已以封装形式经过物理网卡，虚拟交换机流量也会经过物理网卡）。`counted` 中有 `unknown` 时 `may_double_count: true`；接口表读不到时退回 PDH `Network Interface` 求和，`classification: "unavailable"`、全部按 `unknown` 计入并标 `may_double_count: true`。`network.received_bytes_per_sec`/`network.sent_bytes_per_sec` 指标取同一合计，`reason` 即 `network.note`。
- `status_snapshot.network` 是可选字段（契约见 `schemas/v1/protocol.schema.json`）；单次快照与 `status --watch` 的每行 JSONL 都带它。新出现或计数器回退的接口在该区间没有速率，下一次采样起计入。
- 本机实测（Lenovo 83G6，Windows 11 26200，`cargo test -p cm-platform real_snapshot_classifies_interfaces -- --nocapture`）：`WLAN`（类型 71，硬件）→ physical，是唯一计入合计的接口；`Loopback Pseudo-Interface 1`（类型 24）→ 排除；`vEthernet (WSL (Hyper-V firewall))`、`vSwitch (WSL (Hyper-V firewall))` 与三个 `本地连接* 8/9/10`（类型 6，非硬件）→ virtual；第三方隧道驱动 `SakuraiTunnel`（类型 53，非硬件）→ virtual（按类型归为虚拟，同样单独列出）。采样时它有流量、被单独列出，合计只含 `WLAN`，`classification: interface_type`、`may_double_count: false`。本机没有类型 131/23 的活动隧道，隧道分支只由单元测试覆盖。

## 显示偏好（STS-07，桌面）

- “状态 › 系统状态”页的“显示设置”可以勾选显示哪些面板（健康分、指标、网络，至少保留一个）、用“上移/下移”按钮调整顺序（全部是原生复选框、单选与按钮，键盘可达；移动后焦点留在同一行，到达两端时移到另一个移动按钮），以及刷新间隔（2/3/5/10/30 秒）；“进程”页选择的排序也一并记住。
- 保存位置与主题相同：桌面窗口的 `localStorage`（键 `cm.status.prefs`），只属于这台电脑上的本应用，不写共享存储目录，CLI 与 Agent 读不到也不受影响。每次读取都逐字段校验，损坏或未知的值回到默认；存储被阻止时本次打开仍按所选显示，并提示没有保存。两个页面各自只改自己的字段（读—改—写），互不覆盖。

## 概览洞察卡片与旧下载（DIN-03/04）

- `insights overview` 输出 `insight_overview`：旧下载、虚拟机与容器、还原点与快照、休眠/分页文件、组件存储、旧 Windows 安装、更新分发缓存、开发工具缓存、AI 模型库、设备备份等卡片。每张卡片给出已知大小，或 `logical_bytes: null` 加 `size_note`（部分成员无法测量时不把部分和当总数），以及成员中最早的观测时间；`count` 对快照给出还原点数量，大小不可得时只显示数量。`link` 指向磁盘分析位置（`analyze` + 路径）、旧下载审阅或已知对象列表。桌面端调用 `cm_app::insights::overview`（输入已收集时用纯函数 `overview_wire`）。
- `insights downloads [--older-than-days 90] [--limit 50]` 输出 `old_downloads_review`：下载目录（含 3 层子目录，不跟随链接，最多 20000 项）中修改时间早于阈值的文件，按大小排序，`selected` 恒为 `false`；处理只能走手选移入回收站（`trash`）或安装包审阅（`installer scan`），从不是默认候选。

## 磁盘视图的“可手动清理”线索（DIN-05）

- 目录条目可带可选字段 `cleanable_hint`（`entries list` 的 `entry_item`，桌面“分析 › 磁盘”的表格与详情）。它只在当前这一页的真实目录上计算（文件、链接、挂载点与云端边界从不带），不在扫描时计算：每个目录读一次 `CACHEDIR.TAG` 头部；名称是构建产物名时再列一次父目录；出现签名后才加载硬拒绝集合与用户保护名单。核心逻辑在 `cm_core::cleanable`，实时探测在 `cm_platform::cleanable`。
- 依据只有三种可验证的签名，按优先顺序取第一种成立且未被保护的：
  - `build_artifact`：与构建产物清理共用同一识别器（`cm_core::purge::project_markers`/`owned_artifact`）——产物名必须是项目根的直接子目录，且项目根里有该产物的所属标记（如 `node_modules` 旁有 `package.json`、`target` 旁有 `Cargo.toml`）；位于产物目录或点目录之内的（如嵌套依赖、`.vscode\extensions`）不算。`rule_id` 为 `purge.*`，`detail` 为找到的标记，`flow: purge`。
  - `catalog_rule`：位于已启用、文件动作、以整个目录为目标、无年龄或大小阈值、无需管理员的目录规则所覆盖的位置之内（日志等按年龄挑文件的规则不算）。`rule_id` 为该规则，`detail` 为解析出的位置，`flow: manual_trash`。
  - `cachedir_tag`：目录内直接有一个普通本地文件 `CACHEDIR.TAG`，开头恰为 `Signature: 8a477f597d28d172789f06886806bc55`（链接、云占位、签名不符都不算）。`flow: manual_trash`。
- 保护优先：硬拒绝（含祖先与目录内的硬拒绝位置）、默认保护类目（含祖先与带 `.git` 的工作树）、用户保护路径会使线索消失；用户保护了对应规则时只能退到另一种成立的依据。保护名单不可读、父目录无法列出、签名读不到时一律不标记。
- 线索只读：不预选、不生成候选或计划、不改变档位或保护判定。桌面行内显示“可手动清理”小标记（悬停提示与读屏文字给出依据），详情面板列出依据并提供入口：`purge` 打开“清理 › 构建产物”，`manual_trash` 只针对这一项打开移到回收站的审阅（与多选的手选回收站相同流程，先检查保护与占用、批准后才移动），当前选择不受影响。

## macOS

命令与记录形状与 Windows 相同（`status`、`status --watch`、`process list|show|pin|unpin|end`、`plan review|apply`），同样受实验功能开关约束。

- 指标来源（全部只读、公开接口）：CPU 为 `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` 两次读数的节拍差（总体与每核），另有 `getloadavg` 负载；内存为 `host_statistics64(HOST_VM_INFO64)` 页计数与 `hw.memsize`，“已用”= 应用内存（internal − purgeable）+ 联动（wired）+ 压缩，与活动监视器的“已使用内存”一致，另报 `kern.memorystatus_vm_pressure_level`（1 正常、2 警告、4 严重）与 `vm.swapusage`；磁盘容量与剩余为数据卷 `statfs`（不含可清除空间），可清除空间为 `CFURL` 的“重要用途可用容量”减去 `statfs` 可用值；磁盘读写速率与繁忙度来自各 `IOBlockStorageDriver` 的 `Statistics`（繁忙度为请求时间占区间的比例，按盘求和并封顶 100，并发请求会重叠）；GPU 为 `IOAccelerator` 的 `PerformanceStatistics` 中最高的利用率，读不到时为 `unavailable`；电池为 `IOPSCopyPowerSourcesInfo`（电量、供电来源）与 `AppleSmartBattery`（循环次数、设计与满充容量 → 健康度；Apple 芯片的 `MaxCapacity` 是百分比，改用 `AppleRawMaxCapacity`）。
- 温度与风扇只通过系统管理控制器（SMC）与 HID 传感器服务这类私有接口提供，本产品不使用：`thermal.max_zone_celsius`、`gpu.temperature_celsius`、`fan.rpm` 恒为 `unavailable` 并附原因，健康分的温度维度因此不参与计算（权重按其余维度重新归一）。系统盘 SMART/NVMe 健康需要存储驱动的用户客户端，`disk.system.failure_predicted` 为 `unavailable`。
- 僵尸进程（N13）：`process.zombie_count` 为 libproc 快照中状态为 `SZOMB` 的进程数（已退出、父进程尚未回收，不占 CPU 与内存）；只在 macOS 报告。
- 网络（STS-07）：`sysctl(NET_RT_IFLIST2)` 的 `if_msghdr2` 记录给出每个接口的类型、标志与 64 位字节计数；计数回退时，回绕后的差值小于取值范围一半才视为回绕，否则视为重置，该区间无速率。分类：`lo*` 或 `IFF_LOOPBACK` → 回环（排除）；`utun`/`ipsec`/`ppp`/`gif`/`stf` 或点对点接口 → 隧道；`bridge`/`vmnet`/`vmenet`/`vnic`/`vboxnet`/`feth`/`tap`/`anpi`/`awdl`/`llw`/`ap`/`XHC` → 虚拟；类型为以太网或 Wi-Fi 的 `en*` → 物理；其余为未知（计入合计并标 `may_double_count`）。去重规则与 Windows 共用 `cm_core::status::network`。
- 进程列表：libproc 快照（PID、父进程、所有者、启动时间、状态、可执行路径）加每进程 `proc_pid_rusage(RUSAGE_INFO_V2)`（CPU 时间、物理占用即活动监视器的“内存”、磁盘读写字节），速率取 1 秒窗口。没有管理员权限时内核不提供其他用户进程的用量，这些值为 `null` 并附说明，从不写 0；每进程网络需要私有的 NetworkStatistics，恒为 `null`。`started_at` 为启动时间的 Unix 微秒数。
- 发布者：静态代码签名声明的 Team ID（满足 `anchor apple` 的系统代码报告为 Apple）；`signature.reason` 注明是“声明”的 Team ID，页面哈希未校验。用途说明来自 macOS 本地知识表，名称、位置（系统卷或应用包内）与签名者三者一致才显示。
- 结束进程：`process end PID` 生成 `process` 计划（绑定 PID、启动时间、可执行路径与 SHA-256），审阅后 `plan apply` 重新核对启动时间、路径、摘要与保护判定，紧接着再核对一次启动时间后发送 `SIGTERM`，最多等待 5 秒确认退出（成为僵尸也算已退出）。macOS 没有进程句柄，最后一次核对与发信号之间只隔几个系统调用。不提供结束入口、执行时再次拒绝：PID 0/1、`launchd`、`kernel_task`、`WindowServer`、`loginwindow`、CleanManager 自身、安全软件，以及任何其他用户的进程（`other_user`）。
- 供维护诊断等调用方使用的采样入口：`cm_platform::macos::status::sample(window)`（单次快照，速率覆盖 `window`，至少 1 秒）、`snapshot()`、可复用的 `Sampler::new()` / `sample()`，以及 `zombies()`。
