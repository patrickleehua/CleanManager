//! Strict argument parsing: positional command words plus `--key value` options and flags.
//! Unknown or repeated options are errors (except [`REPEATABLE`] ones, which name one more object each time);
//! nothing is guessed.
//!
//! [`COMMANDS`] is the declarative argument table of every command group. Shell completions are
//! generated from it (LIF-03), and unit tests check that the dispatch in `commands.rs` accepts
//! nothing the table does not declare, that no command has a `--yes`-style self-approval flag
//! and that options widening a selection are refused for non-interactive callers (CLI-12).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Format;
use crate::i18n::Lang;

/// Options that take no value.
const FLAGS: &[&str] = &["json", "non-interactive", "dry-run", "all-default-selected", "watch", "detach", "no-color", "no-redact", "end-running", "check", "add-to-path", "remove-user-data", "debug"];

/// Accepted by every command: output format, human-text language, no input waiting, no colour, and the
/// stderr-only diagnostic trace (`--debug`, CLI-14) that never changes stdout or any decision.
pub const GLOBAL_OPTIONS: &[&str] = &["format", "lang"];
pub const GLOBAL_FLAGS: &[&str] = &["json", "non-interactive", "no-color", "debug"];
/// Options that may be given several times, each naming one more object (UNI-11: one uninstall plan for several
/// applications). Every other option given twice is an error.
pub const REPEATABLE: &[&str] = &["instance"];
/// Internal options that are accepted but never offered by completions or documented as user flags.
pub const HIDDEN_OPTIONS: &[&str] = &["host-scan-id"];

/// One command of the argument table.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub words: &'static [&'static str],
    /// Positional argument after the words (`ID`, `SHELL`), if any.
    pub arg: Option<&'static str>,
    pub options: &'static [&'static str],
    pub flags: &'static [&'static str],
    /// Options that widen a selection beyond `default_selected` other than explicit object IDs; refused when the caller
    /// is non-interactive (CLI-12). Explicit `--candidates` IDs are never widening: they name objects and still need approval.
    pub widening: &'static [&'static str],
    /// Creates plans or changes this tool's own settings/files (never a target without an authorized plan).
    pub writes: bool,
    pub summary_en: &'static str,
    pub summary_zh: &'static str,
}

const fn c(words: &'static [&'static str], arg: Option<&'static str>, options: &'static [&'static str], flags: &'static [&'static str], en: &'static str, zh: &'static str) -> CommandSpec {
    CommandSpec { words, arg, options, flags, widening: &[], writes: false, summary_en: en, summary_zh: zh }
}

const fn w(words: &'static [&'static str], arg: Option<&'static str>, options: &'static [&'static str], flags: &'static [&'static str], en: &'static str, zh: &'static str) -> CommandSpec {
    CommandSpec { words, arg, options, flags, widening: &[], writes: true, summary_en: en, summary_zh: zh }
}

const ID: Option<&str> = Some("ID");

/// Every command group and its accepted options. Add a row here when adding a dispatch arm.
pub const COMMANDS: &[CommandSpec] = &[
    c(&["capabilities"], None, &["store"], &[], "Platform, backends and capability limits", "平台、后端与能力限制"),
    w(&["experimental"], None, &["store", "state"], &[], "Show or set (--state on|off) experimental features: tools whose release gate has not passed (off by default)", "查看或设置（--state on|off）实验功能：尚未通过发布门禁的工具（默认关闭）"),
    c(&["scan"], None, &["root", "store", "timeout", "host-scan-id"], &["detach"], "Scan a root (read-only); --detach hosts it in a background process", "扫描指定根（只读）；--detach 由后台进程承载"),
    c(&["scan", "show"], ID, &["store"], &[], "Scan state (running) or result (terminal)", "扫描运行状态或终态结果"),
    c(&["scan", "wait"], ID, &["store", "timeout"], &[], "Wait for a scan's terminal record, at most --timeout seconds", "等待扫描终态，最多 --timeout 秒"),
    c(&["scan", "cancel"], ID, &["store"], &[], "Request a scan to stop (receipt only)", "请求停止扫描（仅回执）"),
    c(&["entries", "list"], None, &["scan", "store", "sort", "kind", "parent", "name", "limit", "cursor"], &[], "Page through directories and large files", "分页查询目录与大文件"),
    c(&["analyze"], None, &["scan", "store", "rules", "preview-rule"], &[], "Rule analysis into a candidate set", "规则分析并生成候选集"),
    c(&["candidates", "list"], None, &["set", "store", "limit", "cursor"], &[], "Page through candidates with reasons and impact", "分页查看候选、理由与影响"),
    c(&["clean", "list"], None, &["rules"], &[], "Cleanup catalog: categories, rules, adapters", "清理目录：类目、规则与适配器"),
    w(&["clean"], None, &["scan", "store", "rules", "mode", "candidates", "volume", "limit", "cursor"], &["dry-run", "non-interactive"], "Preview (--dry-run) or plan catalog cleanup", "预览（--dry-run）或生成清理计划"),
    w(&["plan", "create"], None, &["set", "store", "mode", "candidates"], &["all-default-selected"], "Freeze candidates into a plan", "把候选固定为计划"),
    c(&["plan", "show"], ID, &["store"], &[], "Plan and authorization status", "计划与授权状态"),
    w(&["plan", "review"], ID, &["store"], &[], "Human review and approval in a terminal", "终端内人工审阅与批准"),
    w(&["plan", "revoke"], ID, &["store"], &[], "Revoke a plan's approval", "撤销计划授权"),
    w(&["plan", "apply"], ID, &["store", "rules"], &[], "Apply an authorized plan", "执行已授权计划"),
    c(&["operations", "show"], ID, &["store"], &[], "Operation status or report", "操作状态或报告"),
    c(&["operations", "reconcile"], ID, &["store"], &[], "Reconcile actions without a terminal result", "核对缺终态的动作"),
    c(&["operations", "list"], None, &["store", "limit", "cursor"], &[], "Operation history", "操作历史"),
    c(&["recovery", "list"], None, &["store", "limit", "cursor"], &[], "Quarantined items", "隔离区对象"),
    c(&["quarantine", "list"], None, &["store", "limit", "cursor"], &[], "Alias of recovery list", "recovery list 的别名"),
    c(&["recovery", "show"], ID, &["store"], &[], "One quarantined item", "单个隔离对象"),
    w(&["recovery", "plan", "create"], None, &["store", "restore", "purge"], &[], "Restore or purge plan for quarantined items", "隔离对象的恢复或清空计划"),
    w(&["policy", "create"], None, &["store", "scope", "rules", "mode"], &[], "Create a bounded pre-authorization policy (paused)", "创建有限预授权策略（初始暂停）"),
    c(&["policy", "list"], None, &["store"], &[], "Policies", "策略列表"),
    w(&["policy", "enable"], ID, &["store"], &[], "Enable a policy (terminal only)", "启用策略（仅终端）"),
    w(&["policy", "pause"], ID, &["store"], &[], "Pause a policy", "暂停策略"),
    w(&["policy", "revoke"], ID, &["store"], &[], "Revoke a policy", "撤销策略"),
    c(&["apps", "list"], None, &["limit", "cursor"], &[], "Installed application instances (read-only)", "已安装应用实例（只读）"),
    c(&["apps", "updates"], None, &["store"], &[], "Check for app updates now (winget, Store, Scoop); nothing is sent when checks are off", "立即检查应用更新（winget、商店、Scoop）；关闭检查时不发送任何请求"),
    c(&["apps", "updates", "show"], None, &["store"], &[], "Last update check results (never checks)", "上次更新检查结果（不检查）"),
    w(&["apps", "updates", "config"], None, &["store", "checks"], &[], "Turn update checks on or off", "开启或关闭更新检查"),
    w(&["apps", "update"], None, &["store", "app"], &[], "Plan verified package-manager updates (review required)", "生成经验证的包管理器更新计划（需审阅）"),
    // Application uninstall (UNI-01..10): two separately reviewed plans; nothing runs or is deleted without review.
    c(&["apps", "uninstall", "list"], None, &[], &[], "Uninstall route per instance, or why there is none (read-only)", "各实例的卸载方式或不可卸载原因（只读）"),
    w(&["apps", "uninstall", "plan", "create"], None, &["store", "instance"], &["end-running"], "Plan quitting and uninstalling apps; repeat --instance for several in one plan (review required; --end-running only in a terminal)", "生成退出并卸载应用的计划；重复 --instance 可在一份计划中卸载多个应用（需审阅；--end-running 仅限终端）"),
    c(&["apps", "uninstall", "leftovers"], None, &["store", "operation"], &[], "Grouped leftovers after an uninstall operation (read-only)", "卸载操作后的分组残留（只读）"),
    w(&["apps", "uninstall", "leftovers", "plan", "create"], None, &["store", "operation", "items", "exclude"], &[], "Plan removing leftovers: caches/logs (minus --exclude) plus --items IDs (review required)", "生成残留清理计划：缓存/日志（去掉 --exclude）及 --items 指定对象（需审阅）"),
    c(&["apps", "uninstall", "orphans"], None, &[], &[], "Leftovers of apps removed earlier (review_only, read-only)", "早先移除应用的孤立残留（仅审阅，只读）"),
    w(&["apps", "uninstall", "restore"], None, &["store", "operation"], &[], "Plan restoring registry keys and startup values a leftovers operation removed", "生成恢复残留操作删除的注册表键与启动值的计划"),
    c(&["startup", "list"], None, &[], &[], "Startup and background items (read-only)", "启动项与后台项（只读）"),
    w(&["startup", "disable"], ID, &["store"], &[], "Plan disabling a startup item (review required)", "生成停用启动项的计划（需审阅）"),
    w(&["startup", "enable"], ID, &["store"], &[], "Plan enabling a startup item (review required)", "生成启用启动项的计划（需审阅）"),
    w(&["startup", "remove-stale"], ID, &["store"], &[], "Plan removing a stale Run entry after backup (review required)", "生成备份后删除失效 Run 项的计划（需审阅）"),
    w(&["startup", "restore"], None, &["store", "operation"], &[], "Plan restoring the prior state of a startup operation", "生成恢复启动项操作前状态的计划"),
    w(&["trash"], None, &["scan", "store", "entry"], &[], "Plan moving selected scan entries to the Recycle Bin (review required)", "生成把所选扫描条目移入回收站的计划（需审阅）"),
    c(&["trash", "inspect"], None, &["scan", "store", "entry"], &[], "Coverage, protection and holders of one entry (read-only)", "单个条目的覆盖、保护与占用进程（只读）"),
    c(&["trash", "list"], None, &[], &[], "Items in your Recycle Bin (read-only)", "当前用户回收站中的对象（只读）"),
    w(&["trash", "purge"], None, &["store", "item"], &[], "Plan permanently deleting Recycle Bin items (review required)", "生成永久删除回收站对象的计划（需审阅）"),
    c(&["status"], None, &["interval", "count", "store"], &["watch"], "System status snapshot or JSONL stream (opt-in CPU alerts while streaming)", "系统状态快照或 JSONL 流（流式运行时可选的 CPU 提醒）"),
    w(&["status", "alerts"], None, &["store", "state", "cpu-threshold", "window-seconds", "cooldown-seconds"], &[], "Sustained high-CPU alert settings (off by default)", "持续高 CPU 提醒设置（默认关闭）"),
    c(&["process", "list"], None, &["store", "by", "search", "limit"], &[], "Processes with location, signer, purpose and protection (read-only)", "进程列表：位置、签名、用途与保护（只读）"),
    c(&["process", "show"], Some("PID"), &["store"], &[], "One process with its local facts (read-only)", "单个进程的本地事实（只读）"),
    w(&["process", "pin"], Some("NAME"), &["store"], &[], "Keep a process name at the top of the list", "把进程名置顶"),
    w(&["process", "unpin"], Some("NAME"), &["store"], &[], "Remove a process name from the pins", "取消进程名置顶"),
    w(&["process", "end"], Some("PID"), &["store"], &[], "Plan ending one process (review required; critical processes refused)", "生成结束单个进程的计划（需审阅；关键进程拒绝）"),
    c(&["insights"], None, &[], &[], "Known large system objects with guidance", "已知大型系统对象与处理指引"),
    c(&["insights", "overview"], None, &["older-than-days"], &[], "Overview cards: known size or unknown, with observation time", "概览卡片：已知大小或未知，附观测时间"),
    c(&["insights", "downloads"], None, &["older-than-days", "limit"], &[], "Old downloads for review, largest first, nothing selected", "待审阅的旧下载，按大小排序，无预选"),
    // Optional AI diagnosis (contract diag/1): explains one object, never approves or plans.
    c(&["diag", "config"], None, &["store"], &[], "AI diagnosis providers and key presence (never the key)", "AI 诊断提供方与密钥是否存在（不含密钥）"),
    c(&["diag", "preview"], None, &["scan", "store", "entry", "provider", "origin", "disclose", "question"], &[], "Exact data and destination a diagnosis would send (sends nothing)", "诊断将发送的实际资料与目的地（不发送）"),
    w(&["diag", "create"], None, &["scan", "store", "entry", "provider", "origin", "disclose", "question", "payload-digest"], &[], "Send a reviewed preview (its payload digest) to the configured provider", "把已审阅的预览（凭载荷摘要）发送给已配置的提供方"),
    c(&["diag", "show"], ID, &["store"], &[], "One diagnosis with evidence, grounding and next steps", "单个诊断及其依据、核验与下一步"),
    c(&["diag", "list"], None, &["store", "limit", "cursor"], &[], "Diagnoses kept on this device", "本机保留的诊断"),
    c(&["protect", "list"], None, &["store"], &[], "Protection layers and entries", "保护层与条目"),
    w(&["protect", "add"], None, &["store", "path", "match", "glob", "rule", "task", "note"], &[], "Add a protection entry", "添加保护条目"),
    w(&["protect", "remove"], ID, &["store"], &[], "Remove a user protection entry (terminal only)", "移除用户保护条目（仅终端）"),
    c(&["protect", "check"], None, &["store", "path", "rule"], &[], "Explain the protection decision for a path", "解释路径的保护判定"),
    c(&["purge", "roots"], None, &["store"], &[], "Project scan roots", "项目扫描根"),
    c(&["purge", "roots", "list"], None, &["store"], &[], "Project scan roots", "项目扫描根"),
    w(&["purge", "roots", "add"], None, &["store", "path"], &[], "Add a project scan root", "添加项目扫描根"),
    w(&["purge", "roots", "remove"], None, &["store", "path"], &[], "Remove or exclude a project scan root", "移除或排除项目扫描根"),
    c(&["purge", "scan"], None, &["store", "root", "recent-days", "max-depth"], &["dry-run", "non-interactive"], "Discover build artifacts (preview)", "发现构建产物（预览）"),
    c(&["purge", "preview"], None, &["store", "root", "recent-days", "max-depth"], &["dry-run", "non-interactive"], "Alias of purge scan", "purge scan 的别名"),
    c(&["purge"], None, &["store", "root", "recent-days", "max-depth"], &["dry-run", "non-interactive"], "purge --dry-run: same as purge scan", "purge --dry-run：等同 purge scan"),
    w(&["purge", "plan", "create"], None, &["store", "set", "mode", "candidates"], &["all-default-selected", "non-interactive"], "Plan build-artifact cleanup", "生成构建产物清理计划"),
    c(&["installer", "locations"], None, &["store"], &[], "Declared installer locations", "安装包声明位置"),
    c(&["installer", "scan"], None, &["store", "source", "max-depth"], &["dry-run", "non-interactive"], "Discover installer packages (preview)", "发现安装包（预览）"),
    c(&["installer", "preview"], None, &["store", "source", "max-depth"], &["dry-run", "non-interactive"], "Alias of installer scan", "installer scan 的别名"),
    c(&["installer"], None, &["store", "source", "max-depth"], &["dry-run", "non-interactive"], "installer --dry-run: same as installer scan", "installer --dry-run：等同 installer scan"),
    CommandSpec {
        widening: &["source"],
        ..w(&["installer", "plan", "create"], None, &["store", "set", "mode", "candidates", "source"], &["all-default-selected", "non-interactive"], "Plan installer cleanup (--source only in a terminal)", "生成安装包清理计划（--source 仅限终端）")
    },
    // System maintenance (MNT-01..07): runs are reviewed `maintenance` plans; diagnosis and history are read-only.
    c(&["optimize", "list"], None, &["store", "search"], &[], "Maintenance tasks, applicability and what is not offered", "维护任务、适用性与不提供的操作"),
    w(&["optimize", "plan", "create"], None, &["store", "tasks"], &[], "Plan maintenance tasks (review required; one UAC prompt for admin tasks)", "生成维护任务计划（需审阅；管理员任务一次 UAC）"),
    c(&["optimize", "diagnose"], None, &["window"], &[], "Read-only performance diagnosis (never ends processes)", "只读性能诊断（不结束进程）"),
    c(&["optimize", "history"], None, &["store", "limit"], &[], "Recorded maintenance runs (no undo)", "维护运行记录（无撤销）"),
    c(&["completion", "print"], Some("SHELL"), &[], &[], "Print a completion script to stdout", "向标准输出打印补全脚本"),
    w(&["completion", "install"], Some("SHELL"), &["profile"], &[], "Preview, then append completion to a shell profile after terminal confirmation", "预览补全安装，终端确认后才写入配置文件"),
    c(&["doctor"], None, &["store", "export"], &["no-redact"], "Read-only diagnostic report (redacted; export after preview)", "只读诊断报告（默认脱敏，预览后导出）"),
    // Product lifecycle (LIF-01/02, MVP 14.1): verified self-update, chosen install directory, reviewed self-uninstall.
    w(&["self", "update"], None, &["store", "manifest"], &["check"], "Verify the signed release manifest (--check) or update this copy; package-manager copies get their command", "校验签名发布清单（--check）或更新本副本；包管理器安装给出其命令"),
    w(&["self", "install"], None, &["store", "dir"], &["add-to-path"], "Preview copying this release into --dir; installs only after terminal confirmation, never over files", "预览把本发行版复制到 --dir；终端确认后才安装，绝不覆盖文件"),
    w(&["self", "uninstall"], None, &["store", "install-dir"], &["remove-user-data"], "Reviewed removal plan of CleanManager's own footprint; removes only after terminal confirmation", "CleanManager 自身足迹的移除计划；终端确认后才移除"),
    c(&["help"], None, &[], &[], "Human-readable command list", "人类可读的命令列表"),
];

/// Known values offered by completions (and checked by the commands themselves).
pub const OPTION_VALUES: &[(&str, &[&str])] = &[
    ("format", &["json", "jsonl", "text", "human"]),
    ("lang", &["zh-CN", "en"]),
    ("mode", &["trash", "quarantine", "delete"]),
    ("sort", &["logical_desc", "allocated_desc", "name_asc"]),
    ("kind", &["any", "file", "directory"]),
    ("by", &["cpu", "memory", "disk", "network", "name"]),
    ("checks", &["on", "off"]),
    ("state", &["on", "off"]),
    ("match", &["descendants", "exact", "ancestors"]),
];

/// Option values that are filesystem paths (completions offer files).
pub const PATH_OPTIONS: &[&str] = &["root", "store", "path", "rules", "profile", "export", "volume", "dir", "install-dir", "manifest"];

/// Values of positional arguments that are closed sets.
pub const ARG_VALUES: &[(&str, &[&str])] = &[("SHELL", &["bash", "zsh", "fish", "powershell"])];

/// Option and flag names that would read as self-approval. None may exist on any command (CLI-06/07, CLI-12).
#[cfg(test)]
pub const SELF_APPROVAL_NAMES: &[&str] = &["yes", "y", "assume-yes", "force", "approve", "approved", "auto-approve", "no-confirm", "confirm", "accept", "unattended", "skip-review"];

/// The table row for exactly these command words (positional value excluded).
pub fn spec_for(words: &[&str]) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|s| s.words == words).or_else(|| {
        let (_, head) = words.split_last()?;
        COMMANDS.iter().find(|s| s.arg.is_some() && s.words == head)
    })
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub words: Vec<String>,
    pub options: BTreeMap<String, String>,
    /// Option values exactly as received from the OS (paths may not be valid Unicode).
    pub raw_options: BTreeMap<String, OsString>,
    pub flags: Vec<String>,
    /// Every value of a [`REPEATABLE`] option in command-line order (`options` holds the first).
    pub repeated: BTreeMap<String, Vec<String>>,
    pub format: Format,
    /// `--format jsonl`: one compact record per line, progress records before the terminal one.
    pub stream: bool,
    /// Language of human-readable text only; machine records never change with it (LIF-05).
    pub lang: Lang,
}

// `option`/`flag` serve the plan, entry and recovery commands added in task group 12.
#[allow(dead_code)]
impl Parsed {
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options.get(key).map(String::as_str)
    }

    pub fn raw_option(&self, key: &str) -> Option<&OsString> {
        self.raw_options.get(key)
    }

    /// All values of an option that may be repeated (UNI-11 `--instance`), in command-line order.
    pub fn values(&self, key: &str) -> Vec<&str> {
        match self.repeated.get(key) {
            Some(v) => v.iter().map(String::as_str).collect(),
            None => self.option(key).into_iter().collect(),
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        self.flags.iter().any(|f| f == key)
    }

    /// Non-interactive: asked for explicitly, or no terminal to wait on. Never implies approval.
    pub fn non_interactive(&self) -> bool {
        self.flag("non-interactive") || !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal())
    }

    /// Reject options the command does not understand. Global options are accepted everywhere.
    pub fn allow(&self, options: &[&str], flags: &[&str]) -> Result<(), String> {
        for k in self.options.keys() {
            if !GLOBAL_OPTIONS.contains(&k.as_str()) && !options.contains(&k.as_str()) {
                return Err(format!("unknown option --{k}"));
            }
        }
        for f in &self.flags {
            if !GLOBAL_FLAGS.contains(&f.as_str()) && !flags.contains(&f.as_str()) {
                return Err(format!("unknown option --{f}"));
            }
        }
        Ok(())
    }

    /// CLI-12: a non-interactive caller may only narrow a selection or name objects explicitly.
    pub fn check_selection_widening(&self) -> Result<(), String> {
        let words: Vec<&str> = self.words.iter().map(String::as_str).collect();
        let Some(spec) = spec_for(&words) else { return Ok(()) };
        match spec.widening.iter().find(|o| self.options.contains_key(**o)) {
            Some(o) if self.non_interactive() => {
                Err(format!("--{o} widens the selection and needs an interactive terminal; non-interactive callers name objects with --candidates ID,ID"))
            }
            _ => Ok(()),
        }
    }
}

static NON_INTERACTIVE: AtomicBool = AtomicBool::new(false);

/// `--non-interactive` was given: no prompt may wait for input anywhere in this process (CLI-06).
pub fn non_interactive_requested() -> bool {
    NON_INTERACTIVE.load(Ordering::Relaxed)
}

/// `--lang` value, then `CLEANMANAGER_LANG`, then the system UI language.
fn parse_lang(value: Option<&str>) -> Result<Lang, String> {
    match value {
        Some(v) => Lang::parse(v).ok_or_else(|| format!("unsupported language: {v} (use zh-CN or en)")),
        None => Ok(crate::i18n::detect()),
    }
}

pub fn parse(raw: &[OsString]) -> Result<Parsed, String> {
    let mut words = Vec::new();
    let mut options = BTreeMap::new();
    let mut raw_options = BTreeMap::new();
    let mut flags = Vec::new();
    let mut repeated: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut it = raw.iter();
    while let Some(os_arg) = it.next() {
        let arg = os_arg.to_str().ok_or("command words and option names must be valid Unicode")?;
        if let Some(key) = arg.strip_prefix("--") {
            if key.is_empty() {
                return Err("bare -- is not supported".into());
            }
            if FLAGS.contains(&key) {
                if flags.iter().any(|f| f == key) {
                    return Err(format!("--{key} given twice"));
                }
                flags.push(key.to_owned());
            } else {
                let value = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
                if REPEATABLE.contains(&key) {
                    let v = value.to_string_lossy().into_owned();
                    repeated.entry(key.to_owned()).or_default().push(v.clone());
                    options.entry(key.to_owned()).or_insert(v);
                    raw_options.entry(key.to_owned()).or_insert_with(|| value.clone());
                    continue;
                }
                if options.insert(key.to_owned(), value.to_string_lossy().into_owned()).is_some() {
                    return Err(format!("--{key} given twice"));
                }
                raw_options.insert(key.to_owned(), value.clone());
            }
        } else if arg.starts_with('-') && arg.len() > 1 {
            // No short options: `-y`/`-f` style switches are never guessed into anything.
            return Err(format!("unknown option {arg} (options are spelled --name)"));
        } else {
            words.push(arg.to_owned());
        }
    }
    let lang = parse_lang(options.get("lang").map(String::as_str))?;
    if flags.iter().any(|f| f == "non-interactive") {
        NON_INTERACTIVE.store(true, Ordering::Relaxed);
    }
    let (format, stream) = match (flags.iter().any(|f| f == "json"), options.get("format").map(String::as_str)) {
        (true, Some("text" | "human")) => return Err("conflicting output formats".into()),
        // JSONL is one compact JSON record per line: identical to JSON for single-record commands; streams
        // (`scan`, `status --watch`) emit progress/sample records before the terminal one.
        (true, Some("jsonl")) | (false, Some("jsonl")) => (Format::Json, true),
        (true, _) | (false, Some("json")) => (Format::Json, false),
        (false, Some("text" | "human")) => (Format::Text, false),
        (false, Some(other)) => return Err(format!("unsupported format: {other} (use json, jsonl or text)")),
        // Non-TTY stdout defaults to machine output; never start an interactive UI implicitly.
        (false, None) => (if std::io::stdout().is_terminal() { Format::Text } else { Format::Json }, false),
    };
    Ok(Parsed { words, options, raw_options, flags, repeated, format, stream, lang })
}

#[cfg(test)]
mod tests;
