# Spec Delta

## Purpose

找出下载、桌面、包管理器缓存和聊天/邮件下载中遗留的安装包与磁盘镜像，按来源标注并让用户逐项决定，释放“装完就没用”的空间。

## ADDED Requirements

### Requirement: INC-01 Installer discovery in declared locations
系统 SHALL 在声明的位置（下载、桌面、文档、公共/共享目录、包管理器下载缓存、云盘下载、邮件与常用聊天工具下载目录）以有限深度查找安装包与镜像：macOS 的 dmg/pkg/mpkg/iso/xip，Windows 的 msi/msix/appx 包与可识别的安装程序。不跟随链接，不读取云占位内容。

#### Scenario: Downloads contains a dmg and a pdf
- **WHEN** 运行安装包扫描
- **THEN** 只列出 dmg 及其位置、大小、修改时间，pdf 不出现

### Requirement: INC-02 Archives count only when they carry installers
压缩包 SHALL 仅在其目录中前有限条目包含应用包、安装包或镜像时被视为安装包候选；无法列出目录或加密的压缩包 SHALL 不被视为安装包。可执行安装程序在 Windows 上 SHALL 依据签名/安装程序特征判定，单凭文件名不足。

#### Scenario: A zip contains only documents
- **WHEN** 压缩包内无应用或安装载荷
- **THEN** 它不成为安装包候选

### Requirement: INC-03 Source labels and per-item review
每个候选 SHALL 显示来源标签（下载、桌面、包管理器缓存、邮件、聊天工具等）与是否对应已安装应用的提示。候选 SHALL 默认为 review_only，用户逐项或按来源选择后才生成计划。

#### Scenario: A user filters by source
- **WHEN** 用户只看包管理器缓存
- **THEN** 列表只显示该来源的安装包，选择不影响其他来源

### Requirement: INC-04 Mounted or in-use images are skipped
已挂载的镜像、正在被安装程序使用或打开的文件 SHALL 被跳过并说明；系统 SHALL 提供推出已挂载镜像的只读指引或独立动作，而不是直接删除镜像文件。

#### Scenario: A dmg is currently mounted
- **WHEN** 用户选择该 dmg 清理
- **THEN** 执行时标为 skipped（已挂载），提示先推出

### Requirement: INC-05 Installer cleanup uses plans
安装包清理 SHALL 生成绑定文件身份的固定计划，默认移入回收站，执行前重新核验身份；永久删除需显式选择模式。

#### Scenario: A file is replaced after planning
- **WHEN** 计划生成后同名安装包被新下载覆盖
- **THEN** 执行时该项因身份不符被跳过，新文件保留
