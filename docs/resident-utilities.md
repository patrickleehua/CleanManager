# 常驻工具（Windows 与 macOS）

最近更新：2026-10-17。macOS 的实现与限制见文末 [macOS](#macos) 一节。对应变更 `add-mole-parity-suite` 任务 15.1、15.2（RES-01..04、RES-07）。风扇、充电上限、网络低延迟（15.3，RES-05）与 Touch ID（15.4，RES-06）不在本轮；Windows 上在页面“此平台不提供”中说明原因。实机记录见 [evidence/resident-windows.json](evidence/resident-windows.json)。

默认关闭（设计 D15）：常驻工具尚未通过门禁 G4，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

| 层 | 位置 |
| --- | --- |
| 纯逻辑 | `crates/cm-core/src/resident.rs`：设置、启动项同步与 `turn_off`、唯一的后台循环 `ResidentLoop`、托盘读数、快捷入口白名单、保持亮屏与清洁屏幕参数、长按 Esc、隐私记录判定、电量提醒 |
| Windows 平台 | `crates/cm-platform/src/windows/resident.rs`（启动项值、保持亮屏、隐私记录、电量）、`resident_screen.rs`（清洁屏幕窗口与输入钩子） |
| 桌面宿主 | `apps/desktop/src-tauri/src/resident.rs`（托盘、6 个命令、窗口关闭与退出处理） |
| 界面 | 状态 › 常驻工具（`apps/desktop/src/views/resident.ts`） |

## 托盘（RES-01、RES-07）

- **默认关闭。** 设置保存在存储目录的 `resident.json`（`tray`、`start_at_login`、`battery_alerts`、`battery_threshold`）；文件缺失或无效时按全部关闭处理。
- **开启后**：通知区域出现 CleanManager 图标，菜单只有读数和打开页面——处理器、内存已用、系统盘可用空间、摄像头和麦克风使用情况（不可用时写“不可用”，从不写 0），以及“打开系统状态 / 打开进程 / 打开清理 / 常驻工具设置”“关闭常驻组件”“退出 CleanManager”。快捷入口是固定白名单（`QuickLink`），托盘没有清理、卸载或维护入口。读数由一个后台线程每 5 秒采样一次（复用状态页的 PDH 采样器）。
- 托盘开启时关闭主窗口只是隐藏窗口；托盘菜单的“退出”走正常的安全关闭流程（扫描与执行先安全停止）。
- **登录时启动（可选）**：只有托盘开启后才能打开。它在 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 写入一个值，名称固定为 `CleanManager.Resident`，内容为 `"<桌面程序路径>" --resident`（启动后只显示托盘，不显示窗口）。CleanManager 只读写这一个值名，不触碰 Run 下其他任何值。
- **关闭**（页面开关或托盘菜单“关闭常驻组件”）：先停止并 join 采样线程（`turn_off` 返回时线程已退出，之后不再采样），再删除 `CleanManager.Resident`，最后移除托盘图标。启动时若设置为关闭而该值仍在（例如上次删除失败），会再次删除。
- **生命周期可见**：页面显示状态（已开启/正在采样或已关闭）、启动方式、启动项位置与是否已创建、版本、移除方式。
- **随产品卸载**：`cleanmanager self uninstall` 读取该值，存在时作为 `system_setting` 项 `remove_run_value` 列入计划（摘要覆盖它），执行时按值名删除；不存在时计划说明中写明“not present”。`resident.json` 归入配置，默认保留，`--remove-user-data` 时删除。
- 测试：`ResidentLoop` 用计数采样器验证关闭后计数不再增加；启动项值只写入一次性测试键 `HKCU\Software\CleanManagerTest\<唯一名>`（测试结束删除），从不写真实 Run 键。

## 保持亮屏（RES-02）

- 实现：专用线程调用 `SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)`，结束时调用 `SetThreadExecutionState(ES_CONTINUOUS)`。这是每线程的临时请求（与视频播放器相同），不修改电源计划、息屏或睡眠超时。
- 时长：30 分钟、1 小时、2 小时或“直到结束”（命令接受 1–1440 分钟）。到时、点“结束保持亮屏”、托盘“退出”或程序退出（`RunEvent::Exit`）时撤销；进程意外结束时系统随线程一并撤销。
- 实机：`powercfg /requests`（需要管理员）在保持期间的 DISPLAY 与 SYSTEM 下列出本进程，撤销后消失。

## 清洁屏幕（RES-02）

- 一个黑色、置顶、无边框窗口覆盖整个虚拟屏幕（所有显示器）并隐藏指针；`WH_KEYBOARD_LL` 吞掉按键（只吞掉被吞掉的按下所对应的抬起，开始前已按住的键不会卡住），`WH_MOUSE_LL` 吞掉点击和滚轮，移动指针不受影响。主显示器底部有一行暗灰色提示与倒计时。
- **退出方式**：按住 **Esc 2 秒**；到达超时（界面可选 30 秒 / 1 分钟 / 2 分钟，命令接受 5–300 秒）；Ctrl+Alt+Del 无法被任何程序拦截，始终可用。退出前移除两个钩子，输入随即恢复。同一时间只允许一个清洁屏幕会话。
- 实机：3 秒超时按时退出；注入的 0.5 秒 Esc 被忽略、持续 2.6 秒的 Esc 在 30 秒超时前退出；窗口为置顶、可见、尺寸等于虚拟屏幕。

## 摄像头与麦克风提示（RES-03）

- 来源：`HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{webcam, microphone}` 下每个应用（商店应用以包系列名为键，桌面应用在 `NonPackaged` 下以路径为键，`\` 写作 `#`）的 `LastUsedTimeStart` / `LastUsedTimeStop`。`LastUsedTimeStart` 非 0 且 `LastUsedTimeStop` 为 0 即“正在使用”。只读。
- 显示：桌面应用显示程序文件名，商店应用去掉发布者 ID（`Microsoft.WindowsCamera_8wekyb3d8bbwe` → `Microsoft.WindowsCamera`）。使用结束后提示随下次读取消失。
- 读取时机：常驻工具页打开时每 3 秒；托盘开启时随托盘每 5 秒。只在本机显示，不保存、不上传任何使用历史。记录不存在或不可读时显示“不可用”及原因。

## 电量（RES-04）

- 本机电池：`GetSystemPowerStatus`（电量、是否在充电或使用电源）；没有系统电池时不显示。
- 蓝牙设备：枚举当前存在的设备节点，读取设备属性 `{104EA319-6EE2-4701-BD47-8DDBF425BBE5},2`（字节，0–100）。这是 Windows 蓝牙协议栈为报告电量的设备记录的值，即“设置 › 蓝牙和其他设备”显示的数字；它不在公开 SDK 头文件中声明，因此这里只显示带有该属性的设备，不做估计。没有蓝牙设备时显示“没有已连接的蓝牙设备”，有设备但都没有电量时显示数量并说明不估计。
- 低电量提醒：默认关闭，需要托盘开启（托盘关闭时不在后台检查）。阈值 5–50%（默认 20%）。同一设备低于阈值只提醒一次，正在充电不提醒，回升到阈值 + 5 个百分点后重新计算，设备消失后重新连接时重新计算。提醒显示在托盘菜单读数、常驻工具页与读屏播报中；本版本不注册 Windows 通知（那需要为应用注册 AppUserModelID，属于额外的系统改动）。
- 本机实测：没有已配对的蓝牙设备，因此蓝牙电量路径只有单元测试与“无设备”路径的实机结果。

## 本平台不提供

| 功能 | 原因 |
| --- | --- |
| 风扇控制 | Windows 没有公开的风扇控制接口（厂商工具直接写嵌入式控制器）；读数见“系统状态” |
| 电池充电上限 | Windows 没有统一接口，由厂商工具或固件设置 |
| 网络低延迟 | 需要受限的特权 helper，本版本不安装 |
| Touch ID 管理员认证 | 只在 macOS 提供 |

## 宿主命令

| 命令 | 作用 | 写入 |
| --- | --- | --- |
| `resident_status` | 设置、托盘状态与启动项、上次托盘读数、保持亮屏剩余时间、清洁屏幕参数、本平台不提供的功能 | 无 |
| `resident_save` | 校验并应用设置：开启托盘、同步或删除启动项、关闭托盘 | `resident.json`；`HKCU\…\Run\CleanManager.Resident` |
| `resident_keep_awake` | 开始/结束保持亮屏 | 临时电源请求 |
| `resident_clean_screen` | 显示清洁屏幕直到退出，返回 `esc_held` / `timeout` / `closed` | 无（临时窗口与输入钩子） |
| `resident_privacy` | 摄像头/麦克风使用快照 | 无 |
| `resident_batteries` | 本机与蓝牙设备电量 | 无 |

事件：`resident-navigate`（托盘快捷入口，页面只接受白名单中的四个页面）、`resident-changed`（托盘菜单关闭了常驻组件）、`resident-alert`（低电量提醒）。

桌面依赖：启用 `tauri` 的 `tray-icon` 特性（`tray-icon` 与 `muda` 已在桌面锁文件中，未新增 crate）；`cm-platform` 的 `windows-sys` 新增 `Win32_Devices_Properties` 特性（只读设备属性），测试专用 `Win32_UI_Input_KeyboardAndMouse`（注入 Esc）。

## macOS

对应同一任务 15.2 在 macOS 上的部分（RES-02、RES-03、RES-04，以及 RES-01/07 的登录项）。代码：`crates/cm-platform/src/macos/resident.rs`（及 `resident/tests.rs`）、桌面宿主 `apps/desktop/src-tauri/src/resident.rs` 的 macOS 分支与 `mac_screen` 模块。门禁与 Windows 相同（D15、`resident.utilities`、G4）：实验功能关闭时命令被拒绝，已在运行的托盘、提醒与保持亮屏随之停止。能力行：`resident.utilities` 为 `unverified`（只在托管 runner 上测试过，不是真机），`resident.login_item` 为 `unsupported`。

**验证范围：**下列读数与断言在 GitHub 托管的 `macos-14` arm64 runner（macOS 14.8，虚拟机：没有摄像头、电池和蓝牙设备，只有虚拟声卡）上由 `cargo test` 实际调用；桌面宿主在 CI 上只编译（`cargo check`）；清洁屏幕另在同一 runner 的图形会话里用一次性的临时钩子运行过一次（未提交）：窗口覆盖整个 1920×1080 显示器并获得焦点，截图中程序坞与菜单栏都已隐藏、底部显示提示与倒计时，6 秒超时在约 6.3 秒时以 `timeout` 结束，向页面派发的合成 Esc 按下在约 2 秒后以 `esc_held` 结束。菜单栏状态项没有运行过；没有用真实键盘、多显示器或在任何真机 Mac 上验证。真机证据仍是门禁 G4 的前提。

### 菜单栏（RES-01）

- 与 Windows 共用托盘代码：开启后菜单栏出现 CleanManager 状态项，菜单内容、白名单入口、5 秒采样（macOS 状态采样器 `macos::status::Sampler`）、关闭即停止并 join 采样线程都相同。托盘开启时关闭主窗口只隐藏窗口；点程序坞图标（`RunEvent::Reopen`）或菜单中的页面入口会再显示。
- **登录时启动：不支持。**`SMAppService.mainApp` 只能为签名（并公证）的 app bundle 注册登录项，本构建未签名；CleanManager 也不会改写成隐藏的 `~/Library/LaunchAgents` 作业。`resident_save` 在保存任何内容之前以 `unsupported` 拒绝 `start_at_login`，原因原样显示；启动时若设置文件里有它，按关闭处理并保存。页面的“此平台不提供”列出 `start_at_login_macos`。

### 保持亮屏（RES-02）

- `IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleDisplaySleep, kIOPMAssertionLevelOn, "CleanManager: keep the screen on")`；显示器保持开启期间系统也不会因空闲睡眠。到时、点“结束”、托盘“退出”或 `RunEvent::Exit` 时由专用线程 `IOPMAssertionRelease`；进程意外结束时 powerd 自动撤销。不修改“节能/电池”设置。
- 状态里的 `keep_awake.assertion` 写出当前断言（类型、ID、名称），与 `pmset -g assertions` 的“Listed by owning process”一致。
- 测试（runner）：创建后 `IOPMCopyAssertionsByProcess` 在本进程下列出该名称的断言，释放、到时与 drop 后都不再列出。

### 清洁屏幕（RES-02）

- 每个显示器一个无边框、置顶、黑色背景的 webview 窗口（空白页 `about:blank#cm-clean-screen`，不授予任何 IPC 权限；不用单纯的 `about:blank`，因为 Tauri 对它不发起加载，脚本也就不会执行），页面脚本吞掉按键、点击、滚轮并隐藏指针；按住 **Esc 2 秒**时页面导航到 `cm-clean-screen://esc-held`，由窗口的导航处理器拦截并结束。主显示器底部显示暗灰色提示与倒计时。到达超时（5–300 秒）、用 Command-W 关闭任一窗口时也结束；结束时销毁全部窗口。
- 运行期间本应用设置 `NSApplicationPresentationOptions`：隐藏程序坞与菜单栏、禁用 Command-Tab 切换应用与 Command-H 隐藏（只在主线程、且应用原本是默认展示选项时设置，结束时恢复默认）。**强制退出（Option-Command-Esc）、电源键与锁屏始终可用**，与 Windows 的 Ctrl+Alt+Del 对应。
- **macOS 上 webview 做不到的：**输入只在 CleanManager 是活跃应用时被吞掉——没有辅助功能（Accessibility）权限，任何应用都不能拦截其他应用或系统的输入，本产品也不申请该权限；系统先处理的按键（亮度、音量、调度中心、聚焦搜索、截屏、触控板手势）仍然生效；开始前已按住的键在松开前被忽略；Command-Q 是否被页面吞掉取决于 WebKit，若退出了应用，窗口与展示选项随进程一起消失。这些限制没有在真机上逐项验证（runner 上只验证了覆盖、隐藏程序坞与菜单栏、超时与合成 Esc）。

### 摄像头与麦克风提示（RES-03）

- 摄像头：CoreMediaIO 列出 `kCMIOHardwarePropertyDevices`，逐个读 `kCMIODevicePropertyDeviceIsRunningSomewhere`。任一为真即“正在使用”。**没有公开接口说明是哪个应用在用摄像头**，因此显示为“未知应用（macOS 只报告设备正在使用，不提供应用名称）”，从不猜测。
- 麦克风：CoreAudio 列出 `kAudioHardwarePropertyDevices` 中有输入流（`kAudioDevicePropertyStreams`，输入范围）的设备，读 `kAudioDevicePropertyDeviceIsRunningSomewhere`。macOS 14 起还读 CoreAudio 进程对象（`kAudioHardwarePropertyProcessObjectList`、`kAudioProcessPropertyIsRunningInput`、`kAudioProcessPropertyPID`、`kAudioProcessPropertyBundleID`），只有它们能说明“哪个应用在读输入”，名称取自进程的 `.app` 名或进程名。同时有输入和输出的设备（例如 USB 耳麦）在播放声音时也是“运行中”，此时由进程对象判定；没有进程对象（macOS 13 及更早）时状态为“未知”并说明原因。
- 任一属性读不出时，该项为“未知”并带原因（`OSStatus` 四字符码），整个提示显示为不可用，原因里同时写出另一项的已知状态——不会把读不出的状态显示成“未在使用”。只读，不保存、不上传。
- 测试（runner）：没有摄像头时摄像头为确定的“未在使用”；虚拟声卡空闲时麦克风为“未在使用”；macOS 14 的进程对象可读。纯逻辑测试覆盖耳麦、无进程对象、属性不可读与命名规则。

### 电量（RES-04）

- 本机电池：`IOPSCopyPowerSourcesInfo` 中 `Type = InternalBattery` 的电源（`Current Capacity` / `Max Capacity`，`Is Charging` 或 `Power Source State = AC Power` 视为充电/接电源）。没有内置电池（台式机、虚拟机）或电池不在位时不显示，从不显示 0%。
- 外设：遍历 I/O Registry 中 Apple HID 外设驱动发布 `BatteryPercent` 的条目（`AppleDeviceManagementHIDEventService`，以及旧版的 `AppleBluetoothHIDKeyboard`、`BNBMouseDevice`、`BNBTrackpadDevice`），名称取 `Product`，ID 取 `DeviceAddress`（其次 `SerialNumber`、注册表条目 ID）。`Transport` 为蓝牙的 `IOHIDDevice` 若没有对应的 `BatteryPercent`，列入 `unavailable_devices` 并说明“不估计”；数值不在 0–100 的同样列为不可用。耳机等非 HID 蓝牙设备不在 I/O Registry 中公开电量（读取已配对设备需要蓝牙隐私授权），因此不列出。
- 低电量提醒与 Windows 共用 `BatteryAlerts`（阈值、只提醒一次、回升 5 个百分点后重新计算）。
- 测试（runner）：虚拟机没有电池和蓝牙外设，本机电池为空、外设列表为空；夹具测试覆盖同一设备多处条目的合并、地址写法归一、无电量设备与越界数值。

### macOS 上不提供

| 功能 | 原因 |
| --- | --- |
| 登录时启动菜单栏 | `SMAppService.mainApp` 需要签名的 app bundle；本构建未签名，且不写隐藏的 LaunchAgents 作业 |
| 风扇控制、电池充电上限 | 需要经签名的特权 helper（D12）访问 SMC，本构建不安装 helper；风扇转速也不读取 |
| 网络低延迟 | 需要特权 helper，本构建不安装 |
| Touch ID 管理员认证 | RES-06（任务 15.4）未实现；不修改 `/etc/pam.d` |

宿主命令与 Windows 相同；macOS 上 `resident_status.supported` 为 `true`，`not_here` 使用带 `_macos` 后缀的 ID（原因与 Windows 不同），`resident_batteries` 多出 `unavailable_devices`。桌面页面的部分文字（“登录 Windows 时只启动托盘”“Ctrl+Alt+Del 始终可用”“读取 Windows 的使用记录”）仍按 Windows 书写，需要页面按 `platform` 区分后才适用于 macOS。
