# Windows 缺陷修复复验报告（2026-10-22）

**BUG-WIN-01 已修复并通过独立 CLI 与原生窗口复验。11 项复验中，10 项通过、0 项产品失败、1 项检查受环境阻碍。** 受阻项为 Cloud Files 测试夹具的注册，不能据此宣称目录测试组全绿或云占位行为已验证。

[复验计划与预期](windows-2026-10-22-retest-plan.md) · [逐项运行记录](../evidence/runs/)（`run_win_retest_2026-10-22_rt-*.json`，已脱敏；原始输出仅本地保留在 `.research/acceptance-raw/2026-10-22/windows-2026-10-22-retest-evidence/`）· [初次失败报告](windows-2026-10-22-report.md)

## 修复内容与版本

用户授权的 subagent `fix_windows_file_scan_root` 实施修复，主 agent 审查实际 diff、重新构建产品并执行独立验收。

- [handle.rs](../../crates/cm-platform/src/windows/handle.rs)：从已打开句柄查询实际对象属性，避免只检查一个随后可能被替换的路径。
- [directory.rs](../../crates/cm-platform/src/windows/directory.rs)：能力探测与扫描均验证根是目录；普通文件在写入根记录之前返回 `failed`，issue 为 `root_not_directory`。
- [directory/tests.rs](../../crates/cm-platform/src/windows/directory/tests.rs)：新增真实普通文件根回归，核对 probe 拒绝、扫描失败、零索引写入及内容不变。

基线为 `2d654d9199d4959a7492f4e519158a8255dc0f29`，加以上三个文件的未提交补丁。未 commit/push。实际补丁原文仅本地保留（`code.patch`），补丁摘要及二进制 SHA-256 记录在每条复验运行记录的 `config`（如 [RT-01](../evidence/runs/run_win_retest_2026-10-22_rt-01.json)），不能把基线 commit 单独当作已包含修复的版本。

本轮构建：

```text
cargo build -p cm-cli --locked
npm run tauri -- build --debug --no-bundle
```

两项均成功。桌面构建包含 TypeScript 检查、前端生产构建及 Tauri/Rust 编译；实际启动新二进制，原生 WebView 地址为 `http://tauri.localhost/`，使用内置页面。没有用浏览器预览或 mock 代替原生验收。

环境仍为 Windows 11 x64 / build 10.0.26200。沿用原来的专用合成夹具和 sentinel，并使用新的独立 `CM_STORE_DIR`；最终实验功能恢复关闭。未批准任何计划或执行文件移动、删除、恢复、卸载及维护。

## 修复前后

| 观察点 | 修复前 | 本轮实际观察 |
| --- | --- | --- |
| 1,024 字节普通文件作为根 | partial / 退出 5 | failed / 退出 8，`root_not_directory` |
| 被发现/索引的条目 | 1 项，错误的 directory | 0 项，entries 查询为空 |
| 原生概览 | 部分完成、已知空间 0 B | 失败、大小未知、明确的非目录原因 |
| 正常目录 | 10 项、27,904 字节 | 仍为 completed、10 项、27,904 字节 |

证据：[文件根输出与空索引](../evidence/runs/run_win_retest_2026-10-22_rt-01.json)、[正常目录](../evidence/runs/run_win_retest_2026-10-22_rt-05.json)、[原生失败与正常显示](../evidence/runs/run_win_retest_2026-10-22_rt-09.json)。

## 独立复验结果

| 用例 | 结果 | 实际结果与证据 |
| --- | --- | --- |
| RT-01 普通文件根 | 通过 | failed / 8，root_not_directory，0 条目，空索引。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-01.json) |
| RT-02 空文件根 | 通过 | 0 字节文件同样明确拒绝，未混同于空目录。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-02.json) |
| RT-03 中文空格名文件根 | 通过 | 2,048 字节文件拒绝，原始路径与中文完整，空索引。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-03.json) |
| RT-04 不存在根 | 通过 | 保持 failed / root_unavailable，不回退到其他范围。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-04.json) |
| RT-05 正常/空目录 | 通过 | 普通目录 completed / 10 项 / 27,904 字节；空目录 completed / 1 个根条目 / 0 字节，无 issue。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-05.json) |
| RT-06 查询回归 | 通过 | 5 页各 2 项，无重无漏；中文 1 项、无匹配 0 项；limit=0/501 拒绝；字节为字符串/null。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-06.json) |
| RT-07 取消回归 | 通过 | 400 目录/40,000 文件扫描最终 cancelled / 130，保留 6,501 个已确认条目且可查询。未以取消回执代替终态。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-07.json) |
| RT-08 保护与批准边界 | 通过 | 假凭据项 policy_denied / 3；普通文件生成同一固定回收站计划，TTL 900 秒；未批准 apply 返回 needs_approval / 3，执行历史为空。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-08.json) |
| RT-09 原生窗口 | 通过 | 新二进制显示文件根失败和非目录原因；读取正常扫描时显示完成与精确大小；磁盘页中文过滤仅 1 项，清空后恢复 6 项。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-09.json) |
| RT-10 只读完整性 | 通过 | 所有输入内容/大小/名称/目录清单与原始 SHA-256 完全一致；sentinel 不变；最终没有执行记录。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-10.json) |
| RT-11 平台检查与协议 | 受阻（其余检查通过） | 新永久回归、clippy、diff 检查通过；33 个新机器输出均通过 Schema。目录组另有 Cloud Files 夹具初始化失败；2 个 VHD 用例按既有 opt-in 条件未实际运行。详见下节。 [运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-11.json) |

## 检查结果与剩余限制

[RT-11 运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-11.json) 摘录 subagent 检查记录中的目录组输出及其他命令的运行摘要（原始记录仅本地保留）：

- `cargo test -p cm-platform --lib regular_file_root_is_refused_before_any_directory_is_recorded --locked`：1 passed。
- `cargo clippy -p cm-platform --all-targets --locked -- -D warnings`：退出 0。
- `git -c safe.directory=D:/workplace/cleanmanager diff --check`：退出 0。
- `cargo test -p cm-platform --lib windows::directory::tests --locked -- --nocapture`：原始结果 **12 passed / 1 failed**。其中 2 个 VHD 用例是既有条件提前返回，实际执行并通过的为 10 项，包括新增文件根、原有目录只读、junction 边界、长路径、权限缺口、稀疏压缩大小、取消和自身存储边界。
- Cloud Files 用例在 `CfRegisterSyncRoot` 注册临时夹具时返回 `0x80070001`，尚未调用扫描器。主 agent 在真实 Windows 主机环境重跑该项，仍在同一准备步骤失败，见 [RT-11 运行记录](../evidence/runs/run_win_retest_2026-10-22_rt-11.json)（复跑原始输出仅本地保留）。没有更改 Windows 安全设置、真实云账号、测试期望或跳过逻辑来获得通过。
- 全库 `cargo fmt --all -- --check` 发现大量未修改文件的既有格式差异；没有做全库格式化。本次只保留三个修复文件的相关改动，不把格式检查描述为通过。
- [新输出 Schema 检查](../evidence/runs/run_win_retest_2026-10-22_rt-11.json)：33 个记录、0 错误。结构通过与实际行为通过分别判定。

**BUG-WIN-01 可按本轮所列 Windows 前置关闭。** Cloud Files、未准备的 VHD/其他文件系统、初验受阻的完整旧下载洞察、实际删除/恢复与真实系统变更/签名发布，仍未获得新的通过证据。本轮没有将全部原 25 条用例重新执行，也不覆盖整个产品的发布门禁。
