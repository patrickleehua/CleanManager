# 常驻工具（Windows）

最近更新：2026-10-15。对应变更 `add-mole-parity-suite` 任务 15.1、15.2（RES-01..04、RES-07）。风扇、充电上限、网络低延迟（15.3，RES-05）与 Touch ID（15.4，RES-06）不在本轮；Windows 上在页面“此平台不提供”中说明原因。实机记录见 [evidence/resident-windows.json](evidence/resident-windows.json)。

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
