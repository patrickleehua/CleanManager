# Spec Delta

## Purpose

提供菜单栏/托盘常驻面板与日常小工具（状态速览、保持亮屏、清洁屏幕、隐私提示、电量提醒），并在硬件和权限边界内开放风扇、电池养护、网络与 Touch ID 等特权设置。常驻组件由用户主动开启且可完全关闭。

## ADDED Requirements

### Requirement: RES-01 Opt-in resident panel
系统 SHALL 提供可选的 macOS 菜单栏/Windows 托盘常驻组件，显示用户选择的关键指标并可展开小面板；默认关闭或在首次使用时由用户选择开启，关闭后 SHALL 不再后台采样或运行定时任务。常驻组件 SHALL 不执行清理、卸载或维护写动作。

#### Scenario: A user disables the tray component
- **WHEN** 在设置中关闭常驻组件
- **THEN** 托盘图标消失，状态采样与提醒停止，开机启动项被移除

### Requirement: RES-02 Keep screen on and clean screen
常驻面板 SHALL 提供保持亮屏（可设时长，结束或退出时恢复系统设置）与清洁屏幕模式（临时遮蔽显示并屏蔽键盘输入，提供明确的退出方式与超时自动退出）。

#### Scenario: Clean screen mode times out
- **WHEN** 用户进入清洁屏幕后未执行退出操作
- **THEN** 到达超时后自动退出并恢复输入

### Requirement: RES-03 Privacy indicator
系统 SHALL 在平台支持时提示当前正在使用摄像头或麦克风的应用，仅本地显示，不记录或上传使用历史；平台不提供可靠来源时显示不可用。

#### Scenario: An app starts using the microphone
- **WHEN** 麦克风开始被某应用使用
- **THEN** 面板显示该应用名称，停止使用后提示消失

### Requirement: RES-04 Device battery alerts
系统 SHALL 显示本机与可读取的已配对蓝牙设备电量，并支持用户开启的低电量提醒及阈值设置。

#### Scenario: A paired keyboard is low
- **WHEN** 蓝牙键盘电量低于设定阈值
- **THEN** 发出一次提醒，电量恢复后重置

### Requirement: RES-05 Privileged hardware controls behind a restricted helper
风扇控制、电池充电上限（养护）与网络低延迟等需要特权的设置 SHALL 只通过签名、版本匹配且仅接受固定命令集的受限 helper 执行；硬件或系统版本未验证时 SHALL 显示不可用。退出、卸载 helper 或检测到异常温度时 SHALL 恢复系统自动控制。

#### Scenario: Fan control on unverified hardware
- **WHEN** 当前机型不在已验证列表
- **THEN** 风扇控制不可用并说明原因，只显示读数

#### Scenario: Temperature exceeds the safety threshold under manual fan control
- **WHEN** 手动风扇设置期间温度超过安全阈值
- **THEN** 立即恢复系统自动控制并通知用户

### Requirement: RES-06 Touch ID for administrator prompts on macOS
系统 SHALL 在 macOS 提供启用/停用/查看命令行管理员认证使用 Touch ID 的功能，修改前备份相关配置并验证语法，失败时恢复原配置；Windows 显示不适用。

#### Scenario: Enabling fails validation
- **WHEN** 写入后的配置验证失败
- **THEN** 恢复原配置，报告失败，不留下半修改状态

### Requirement: RES-07 Resident component lifecycle is visible
常驻组件与特权 helper SHALL 在设置中显示安装状态、版本、启动方式与移除入口；移除本产品时 SHALL 一并移除它们并恢复其修改过的系统设置。

#### Scenario: The product is uninstalled with battery care enabled
- **WHEN** 用户卸载产品
- **THEN** 充电上限恢复系统默认，helper 与启动项被移除
