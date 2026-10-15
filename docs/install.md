# 安装、更新与卸载（CLI，Windows）

更新：2026-10-15。覆盖 `define-cleanmanager-mvp` 14.1（Windows 部分）与 14.3 的存储迁移，`add-mole-parity-suite` 17.1/17.2。命令与记录形状以 [CLI 协议](cli-protocol.md) 为准；本地数据与网络访问见 [隐私说明](privacy.md)。平台代码签名、安装器与公证属于 14.2，**本仓库尚未提供签名的发行包**。

## 发行布局

`python -I tools/package_release.py` 在 `target/release-dist/` 生成 `cleanmanager-<版本>-windows-x86_64.zip` 及同名目录：

| 文件 | 说明 |
| --- | --- |
| `cleanmanager.exe` | 独立 CLI（CLI-01）：不依赖桌面包、窗口或源码树 |
| `rules\windows\*.json`、`rules\registry\*.json` | 随包规则包与实测登记（不含 `rules/examples`） |
| `cleanmanager-release.json` | 发行清单：产品、版本、目标、`install_source` 与每个文件的 SHA-256/大小；自更新与自卸载据此判断哪些文件属于本工具 |
| `THIRD-PARTY-NOTICES.txt` | 第三方许可与声明原文（[生成方式](third-party-notices.md)） |
| `PRIVACY.md`、`README.txt` | 隐私说明与简短使用说明 |

规则包查找顺序：`CM_RULES_DIR` → 程序旁的 `rules` 目录 → 仅调试构建回退到编译时的仓库 `rules`。发行构建不嵌入也不读取源码路径；打包脚本用 `--remap-path-prefix` 去掉构建机路径，并检查程序与文本文件中不含仓库路径、用户目录、用户名与调试专用的测试密钥变量名。包内不含研究制品（`.research`、`docs/evidence`、`output`、`examples`、测试）。

## 安装到自选目录

- 直接把 zip 解压到任意目录即可运行；不会修改 PATH 或注册表。
- `cleanmanager self install --dir D:\Tools\CleanManager` 从一个完整的发行布局（每个文件哈希都与清单一致）复制到你选的**新或空目录**；目录非空或已有 CleanManager 安装时为 `conflict`，从不覆盖任何文件。JSON、非交互或无终端时只返回 `self_install_result` 预览，终端内输入 `install` 后才复制；复制失败会删除本次创建的全部内容。
- 冲突处理：预览列出 PATH 上每个 `cleanmanager(.exe/.cmd/.bat/.ps1)` 及其与本发行版是否相同（`same_release`/`different`）。不同的程序**从不被替换或删除**；`--add-to-path` 只在不存在不同程序时向**用户** PATH 末尾追加安装目录（保留 `REG_EXPAND_SZ` 类型并广播环境变更），否则为 `refused_conflict` 且 PATH 不变。追加的确切条目记录在安装目录的 `cleanmanager-install.json`，卸载时只移除这一条。

## 更新（LIF-01）

`cleanmanager self update --check` 只检查；`cleanmanager self update` 校验后替换。通道来自构建时的 `CM_UPDATE_MANIFEST_URL`，或用 `--manifest` 指定 HTTPS 地址或本地文件（离线更新）。

1. 信封 `cleanmanager-update-envelope/1`：`payload_base64` 为清单原始字节，`signatures[]` 为 `{key_id, algorithm: "ed25519", signature_base64}`。签名必须由构建时 `CM_UPDATE_PUBLIC_KEYS`（逗号分隔的十六进制 Ed25519 公钥）中的密钥验证；`key_id` 为公钥 SHA-256 的前 16 个十六进制字符。没有嵌入公钥的构建一律 `cannot_check`，不安装任何东西。调试构建另外接受环境变量 `CM_UPDATE_TEST_PUBLIC_KEY`（供测试用测试密钥驱动真实程序），发行构建中这段代码不存在（打包脚本核对）。
2. 清单 `cleanmanager-update/1`：`product`、`version`（语义化版本，只升不降）、`compat.index_schema/audit_schema`（新程序能读取的存储版本；低于本机存储版本时为 `incompatible`）与 `targets.<目标>.files[]`（`path`、`url`、`sha256`、`size`；路径受限于安装目录，`url` 为 HTTPS 或相对清单位置）。
3. 逐文件下载到安装目录内的暂存目录，大小与 SHA-256 在内存与落盘后各核对一次；不符即删除暂存并报告 `verification_failed`，当前版本原样可用。
4. 暂存的新程序必须能启动并通过 `capabilities` 报告清单中的版本。
5. 逐个顶层条目改名替换，旧文件移入 `.cleanmanager-update-backup\<旧版本>`（保留到下次更新，可手工回退）；中途失败全部改回（`rolled_back`）。随后由新程序在首次打开存储时执行迁移（见下）。

winget、Scoop、npm、cargo、Homebrew 安装（按程序位置或发行清单的 `install_source` 判断）只返回 `use_package_manager` 与对应命令（如 `winget upgrade --id <Id> --exact`、`scoop update <app>`），不访问网络、不改动文件。没有发行清单的开发构建或单独拷贝的程序为 `cannot_check`。

**单实例锁**：`self update`/`self install`/确认后的 `self uninstall` 独占 `<store>\lifecycle.lock`（另在安装目录加锁防止同一副本被并发更新）；其他写命令共享持有。更新进行中新的写命令立即 `conflict`，写操作进行中更新为 `locked`。

**发布签名（14.2 前的流程设计）**：`package_release.py` 同时输出未签名的 `*.update-payload.json`（文件 `url` 相对清单为 `files/<path>`）。发布者用离线保管的 Ed25519 私钥签名原始字节（例如 OpenSSL 3：`openssl pkeyutl -sign -rawin -inkey release-ed25519.pem -in payload.json -out payload.sig`），再用 `python -I tools/package_release.py envelope --payload payload.json --signature payload.sig --public-key-hex <公钥>` 组装信封。私钥不进入仓库或构建机。

**桌面更新器（仅设计）**：桌面宿主当前未配置 Tauri updater（`tauri.conf.json` 中 `bundle.active` 为 false，无 updater 插件），因此桌面端没有自更新。启用时应：使用 Tauri updater 的 minisign 签名（公钥写入 `tauri.conf.json`，与 CLI 的 Ed25519 发布密钥分开保管），并在安装前额外执行本节第 2 步的存储兼容检查、取得同一生命周期独占锁、停止接收新写计划；平台代码签名（Authenticode）与更新签名分别验证。

## 存储迁移（IDX-05、REC-04、GUI-07）

索引库与审计/恢复库各有 `meta.schema_version`。新程序打开旧版本存储时，`crates/cm-store/src/migrate.rs` 在应用当前结构之前逐步迁移：全部步骤与版本号更新在同一个 `BEGIN IMMEDIATE` 事务中，失败整体回滚、存储保持旧版本且旧程序仍可打开；审计库在迁移前先以 `VACUUM INTO` 写出完整副本 `audit.db.pre-v<N>-<时间>.bak` 并保留；每次尝试（开始/完成/失败/拒绝）追加到存储目录的 `migrations.log`。操作、动作日志与结果、隔离记录、注册表/启动项备份、计划与授权在迁移前后做指纹比对，任何迁移若改变它们即回滚——旧的待核对操作不会因迁移而被重新执行。比程序更新的版本、非数字版本或缺少迁移步骤的版本一律拒绝且不做任何改动；无法迁移的索引按“需重建”报告，重建不触及审计库。当前两库均为版本 1，尚无实际迁移步骤；框架与回滚行为由合成迁移测试覆盖。

## 卸载（LIF-02）

`cleanmanager self uninstall` 显示计划（`self_remove_preview`，含计划摘要）；只有终端内输入 `uninstall` 且重新生成的计划摘要不变时才执行。

- **移除**：发行清单中哈希未变的程序文件（之后被修改的文件保留并提示）、发行/安装记录、更新备份与暂存残留及由此变空的目录；缓存（可重建的扫描索引与上次预览）；本工具做过的系统改动——`--add-to-path` 记录的用户 PATH 条目、加载**本副本**的 shell 补全块。正在运行的程序先改名，退出后由一个分离的 `cmd.exe` 删除并移除空目录。
- **默认保留**：配置、保护名单、操作历史（含注册表/启动项备份）与隔离区。隔离区非空时显示数量与大小，并给出恢复（`recovery plan create --restore`）、清空（`--purge`）或保留后卸载的选择；卸载从不删除隔离对象。`--remove-user-data` 只在隔离区为空且没有待核对操作时生效，额外移除已识别的配置/保护/历史文件与 AI 诊断密钥；不认识的文件始终保留。
- **常驻组件**：本构建不安装服务、计划任务、helper 或后台进程。唯一可能创建的启动项是桌面托盘的可选值 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\CleanManager.Resident`（只在开启“登录 Windows 时只启动托盘”后存在，见[常驻工具](resident-utilities.md)）；存在时作为系统设置项列入卸载计划并删除，计划说明中写明是否存在。不修改其他系统设置。包管理器安装只给出其卸载命令；桌面包由其自身安装器卸载。
