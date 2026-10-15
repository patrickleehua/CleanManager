# Spec Delta

## Purpose

在应用管理中集中显示已安装应用的可用更新，并只通过可验证的官方通道执行更新，无法验证时把用户带到官方页面，而不是自行下载未知安装包。

## ADDED Requirements

### Requirement: UPD-01 Multi-channel version detection
系统 SHALL 按应用来源检测可用版本：macOS 支持 Sparkle 订阅、App Store、Homebrew 与 Electron 更新源；Windows 支持 winget、商店与 Scoop。每条结果 SHALL 显示当前版本、可用版本、检测通道与检测时间；无法检测的应用 SHALL 显示“无法检查”及原因。

#### Scenario: An app has no detectable channel
- **WHEN** 应用既无更新订阅也不属于任何包源
- **THEN** 列表显示无法检查，不显示为“已是最新”

### Requirement: UPD-02 Updates run only through verified channels
更新执行 SHALL 仅通过对应通道的官方机制（包管理器升级、商店更新、签名校验通过的 Sparkle/更新源包），执行前验证下载包签名与原安装发布者一致。无法验证或替换时 SHALL 打开应用自身或官方页面，不安装其他来源的安装包。

#### Scenario: The downloaded package signer differs
- **WHEN** 更新包发布者与已安装应用不一致
- **THEN** 更新被拒绝并说明签名不匹配，原应用不受影响

### Requirement: UPD-03 Running apps and batch updates
被更新应用运行中时，系统 SHALL 先请求退出或提示用户稍后更新；批量更新 SHALL 逐项报告成功、跳过、失败，不因单项失败中止其他项。

#### Scenario: One of three updates fails
- **WHEN** 批量更新中一个包管理器升级失败
- **THEN** 另外两项继续，报告显示 1 项失败及错误摘要

### Requirement: UPD-04 Update checks respect network and privacy settings
版本检测 SHALL 由用户触发或在用户开启的周期内运行，可整体关闭；检测请求 SHALL 只发送通道所需的应用标识与版本，不上传应用清单到本产品服务。

#### Scenario: Update checks are disabled
- **WHEN** 用户关闭更新检查
- **THEN** 软件页不发起任何网络检测，显示上次结果的时间或“未检查”
