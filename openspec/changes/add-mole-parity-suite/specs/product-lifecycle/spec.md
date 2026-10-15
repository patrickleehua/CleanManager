# Spec Delta

## Purpose

覆盖 CLI 与桌面的安装后生命周期：可信自更新、干净卸载、终端补全、可贴进问题反馈的只读诊断报告与多语言界面，让用户可以长期、放心地维护产品本身。

## ADDED Requirements

### Requirement: LIF-01 Verified self-update
CLI 与桌面的自更新 SHALL 在替换前校验发布签名与校验和，校验失败即停止且保留当前版本；同一时刻只允许一个更新/安装过程。通过系统包管理器安装的副本 SHALL 提示使用该包管理器更新，不自行覆盖。

#### Scenario: The checksum does not match
- **WHEN** 下载包校验和与发布清单不一致
- **THEN** 更新中止，当前版本继续可用，报告校验失败

#### Scenario: The CLI was installed by a package manager
- **WHEN** 用户运行自更新
- **THEN** 提示对应包管理器的升级方式，不覆盖其管理的文件

### Requirement: LIF-02 Self-uninstall preserves user data by choice
产品自卸载 SHALL 列出将移除的程序、常驻组件、helper 与缓存，并默认保留用户配置、保护名单、操作历史与隔离区；隔离区仍有内容时 SHALL 提醒先恢复或清空，不静默删除。

#### Scenario: Quarantine still has items
- **WHEN** 用户卸载产品时隔离区非空
- **THEN** 显示隔离对象数量与大小，提供恢复/清空/保留后卸载选项

### Requirement: LIF-03 Shell completion
CLI SHALL 为 bash、zsh、fish 与 PowerShell 生成补全脚本，并支持只输出到标准输出或经确认写入用户配置文件；写入前显示变更内容。

#### Scenario: Preview completion installation
- **WHEN** 用户请求预览安装 PowerShell 补全
- **THEN** 显示将追加到配置文件的内容，不修改文件

### Requirement: LIF-04 Read-only Doctor report
系统 SHALL 生成只读诊断报告，包含版本、平台、能力矩阵、权限状态、配置有效性、最近错误码与存储完整性检查；默认脱敏路径与用户名，导出前预览，且不包含凭据或文件内容。

#### Scenario: A user exports a Doctor report
- **WHEN** 生成报告供问题反馈
- **THEN** 预览显示脱敏后的内容，确认后才写出文件

### Requirement: LIF-05 Localized interface with stable machine output
桌面与 CLI 人类可读文本 SHALL 支持至少简体中文与英文，跟随系统语言并可手动切换；机器 JSON 字段、错误码与状态值 SHALL 不随语言变化。

#### Scenario: The UI language is switched to English
- **WHEN** 用户切换语言
- **THEN** 界面文字更新，同一 CLI JSON 输出的 code 与 status 保持不变
