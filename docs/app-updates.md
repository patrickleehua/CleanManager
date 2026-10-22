# 应用更新

状态：变更 `add-mole-parity-suite` 任务 10.2–10.3 已勾选；10.1（检测通道）的 Windows 通道与 macOS 通道（Sparkle、App Store、Homebrew、Electron）都已实现，macOS 部分只在托管 CI 上用夹具验证、未在真机验证，因此 10.1 未勾选。整体未通过 G3 门禁（任务 18.3），没有发布。实现位置：核心 `cm_core::updates`（macOS 判定在 `cm_core::updates::mac`），服务 `cm_app::updates`，平台 `windows/updates.rs` 与 `macos/updates.rs`，执行走 `cm_app::plans` 的 `update` 计划。命令形状以 [CLI 协议](cli-protocol.md) 为准。

默认关闭（设计 D15）：应用更新尚未通过门禁 G3，所以它的 CLI 命令组、对应种类计划的审阅与执行、桌面页面和宿主命令都拒绝使用并说明原因。只为在这台电脑上评估时，用 `cleanmanager experimental --state on` 或桌面“设置 › 实验功能”开启（两者共用一个设置，见 [CLI 协议](cli-protocol.md) 的“实验功能”）；开启后能力仍报告为 `unverified`，不算已发布。

## 检测通道与“无法检查”（UPD-01）

| 通道 | 检测方式 | 结果 |
| --- | --- | --- |
| winget | 运行 App Installer 包内的 `winget.exe list`（无 shell、无窗口、限时、输出有上限），按显示列宽解析表格（任意界面语言） | 来源为 `winget` 的行：`update_available` 或 `up_to_date`；`msstore` 来源的行归商店通道 |
| Microsoft Store | MSIX 包与 winget 标为 `msstore` 的行 | 商店自行检查其应用：一律 `cannot_check` 并给出 `ms-windows-store://downloadsandupdates`（`channel_id` 为商店产品 ID）；`msstore` 行没有“可用”版本不代表已是最新，有可用版本时只写进原因，从不为 `up_to_date` 或包管理器更新（构造函数同样强制） |
| Scoop | 只读本地 Scoop 目录：已安装 `manifest.json`/`install.json` 与所在 bucket 的清单，不联网 | 与本地 bucket 副本比较，说明中写明该副本的更新时间 |
| 自更新应用（Squirrel/Electron） | 安装目录有 `Update.exe` 与 `app-*`，或 `resources\app-update.yml` | `cannot_check`：其更新源无法验证，请在应用内更新 |
| 其他 | — | `cannot_check`：“没有更新通道” |

每条结果含当前版本、可用版本、通道、通道内标识与检查时间。`up_to_date` 只在某个通道实际回答时出现（构造函数与 Schema 都强制），winget 查询失败时是带原因的 `cannot_check`，从不显示为“已是最新”。

## macOS 检测通道（UPD-01）

| 通道 | 检测方式 | 结果 |
| --- | --- | --- |
| Homebrew | 安装了 Homebrew 包时运行一次 `brew outdated --json=v2`（`/opt/homebrew/bin/brew` 或 `/usr/local/bin/brew`；固定 argv、无 shell、清空环境并设 `HOMEBREW_NO_AUTO_UPDATE=1`、`HOMEBREW_NO_ANALYTICS=1`、`HOMEBREW_NO_ENV_HINTS=1`，限时、输出有上限）。卷宗（cask）来自应用清单的 Caskroom 归属；按需安装的公式（formula）从 Cellar 的 `INSTALL_RECEIPT.json` 加入列表 | 被列出：`update_available`（`channel_id` 为 `cask:<token>` / `formula:<name>`）；未被列出：`up_to_date`，说明中写明比较的是 Homebrew 本地索引（自动更新已关闭，`brew update` 刷新）；被 `brew pin` 的只报告不提供升级；查询失败或 `brew` 被拒：`cannot_check` 并给出原因 |
| Homebrew 自更新卷宗 | 卷宗元数据标记 `auto_updates` 或 `version :latest`（`brew outdated` 不跟踪它们） | 不把 Homebrew 的沉默当作答案：有 Sparkle 订阅则按 Sparkle 检查，否则 `cannot_check` 并转到应用自身 |
| Sparkle | `Info.plist` 的 `SUFeedURL`、`SUPublicEDKey`、`CFBundleVersion`、`CFBundleShortVersionString`；只在用户触发检查时用 https 获取该订阅（不附加任何参数或系统信息），用系统 `NSXMLDocument` 解析（从不加载外部实体） | 取默认频道、适用于 macOS 且满足系统版本范围的最新条目，按 Sparkle 的版本规则比较 `sparkle:version` 与 `CFBundleVersion`，显示 `sparkle:shortVersionString`：`update_available` 或 `up_to_date`（较新版本需要更新的 macOS 时在说明中写明）。只报告状态：应用自己用其 EdDSA 公钥校验并安装更新，CleanManager 从不下载或安装（去向为打开应用）。无 `SUFeedURL`（运行时决定订阅）、非 https 订阅、获取或解析失败：`cannot_check` 并给出原因 |
| Mac App Store | 应用包内有 `Contents/_MASReceipt/receipt` | `cannot_check`：App Store 自行检查其应用，去向 `macappstore://showUpdatesPage` |
| Electron | `Contents/Frameworks/Electron Framework.framework` 加 `Contents/Resources/app-update.yml`（electron-updater，说明中写出 provider）或 `Squirrel.framework`（Squirrel.Mac） | 只检测：`cannot_check`，说明中写明应用自己的更新器，去向为打开应用；不读取、不访问其更新源 |
| 系统组件 | `/System/Applications` 或只读系统卷上的应用 | `cannot_check`：随 macOS 通过“软件更新”更新 |
| 其他 | — | `cannot_check`：“No update channel”，从不显示为“已是最新” |

`brew` 的信任规则：Homebrew 的前缀属于安装它的用户而不是 root，所以安全工具运行器对 `brew` 使用“用户所有”规则——文件及其上每一级目录必须属于 root 或当前用户，且其他人不可写；组可写只接受 `admin`（gid 80）与 `wheel`（gid 0），Homebrew 安装器会授予它们，而其成员本就能通过 `sudo` 成为 root。运行器按解析后的真实路径执行工具，链接形式的 `brew`（Intel 上的 `/usr/local/bin/brew` → `../Homebrew/bin/brew`）会让 Homebrew 推算出错误的前缀，因此也被拒绝；被拒时 Homebrew 应用为 `cannot_check` 并给出原因。

macOS 的 `apps update` 只把 Homebrew 升级放进 `update` 计划：每个应用一个 `tool_adapter` 动作，固定 argv `brew upgrade --cask <token>` 或 `brew upgrade --formula <name>`（名称只允许字母数字与 `@._+-/`，不能以 `-` 开头），绑定 `brew` 的 SHA-256。Homebrew 自己按其索引中的校验和验证下载（设计 D8），所以没有单独的“下载后核对签名者”步骤；应用运行中（有进程的可执行文件在应用包或公式目录内）则跳过；卷宗升级后若应用包的签名者（Team ID）与升级前不同，结果为 `signature_changed` 失败；最后回读 Caskroom/Cellar 的版本目录确认。App Store 应用转到 App Store 的更新页，Sparkle/Electron 应用转到应用自身。

测试：`CM_UNINSTALL_TEST_ROOT`/`CM_UNINSTALL_TEST_HOME` 重定向清单时，`brew` 只取 `CM_UPDATES_TEST_BREW`（替身脚本），订阅只读 `CM_UPDATES_TEST_APPCASTS` 目录中的文件，从不联网、从不运行真实的 Homebrew。

## 检查开关与隐私（UPD-04）

检查只在用户执行 `apps updates`（或桌面中点击检查）时运行，没有周期任务。`apps updates config --checks off` 后不运行任何通道、不发出任何请求，`apps updates` 与 `apps updates show` 只显示上次结果及其原始检查时间，没检查过的为 `not_checked`。检查只调用包管理器自己的查询（winget 访问其已配置的源；macOS 上 `brew outdated` 不自动更新、Sparkle 只获取应用自己声明的订阅地址），不向本产品的任何服务上传应用清单。设置与上次结果保存在存储目录的 `app-updates.json`。

## 经验证的更新执行（UPD-02/03）

`apps update --app ID[,ID...]` 把可验证的包管理器更新放进一个 `update` 计划（每个应用一个 `tool_adapter` 动作：固定可执行文件及其 SHA-256、固定 argv、`preview_digest` 绑定目标应用/版本/安装签名者），其余应用给出去向：商店应用 → 商店的下载与更新页，自更新应用 → 应用自身，安装文件没有可验证签名者 → 官方页面。计划只能在终端 `plan review` 中批准，从不被策略匹配。

`plan apply` 对每一项依次：核对目标摘要与 argv 是固定形状 → 核对包管理器可执行文件摘要未变 → 应用正在运行（或运行状态未知）则跳过并请用户关闭后重试（CleanManager 不为更新而关闭应用）→ 下载该确切版本的安装包到新的临时目录（`winget download --version`；Scoop 为 `scoop download`）→ 下载包的已验证签名者必须与已安装应用的签名者一致（忽略大小写、标点与 Inc./Ltd. 等法律后缀），不一致为 `signature_mismatch`、无签名为 `unverifiable`，两者都不安装、原应用不变并指向官方页面 → 运行固定版本的升级（winget 用同一清单的安装包哈希校验）→ 回读已安装版本，相符才算成功，否则为 `unknown`。批量中某一项失败不影响其他项，报告逐项给出成功、跳过、失败与错误摘要。

```text
cleanmanager apps updates --json                      # app_update_list：检查（开关开启时）
cleanmanager apps updates show --json                 # 只看上次结果，从不检查
cleanmanager apps updates config --checks off         # app_update_settings：关闭后零网络请求
cleanmanager apps update --app app_0123,app_4567      # app_update_plan：update 计划 + 其余应用的去向
cleanmanager plan review up_demo                      # 终端内审阅
cleanmanager plan apply up_demo --json                # 逐项验证签名后升级
```

限制：已安装应用的签名者来自安装目录顶层可执行文件（跳过卸载程序与更新器）；注册表未记录安装位置的应用（常见于 MSI 运行库）无法验证，只能前往官方页面。Scoop 的压缩包没有签名，因此 Scoop 更新多数会以 `unverifiable` 跳过。
