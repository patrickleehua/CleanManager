//! Human-readable text in Simplified Chinese or English (LIF-05). Only `--format text` output,
//! prompts and help are localized; machine records (kind, code, status, state and every other
//! JSON field) are byte-for-byte identical in every language, so callers branch on typed values.
//!
//! Language: `--lang zh-CN|en`, else `CLEANMANAGER_LANG`, else the system UI language, else English.

use std::sync::OnceLock;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhCn,
}

impl Lang {
    pub fn parse(s: &str) -> Option<Lang> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "en" | "en-us" | "en-gb" | "english" => Some(Lang::En),
            "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" | "chinese" => Some(Lang::ZhCn),
            _ => None,
        }
    }
}

/// `CLEANMANAGER_LANG`, then the OS UI language. Unsupported languages fall back to English.
pub fn detect() -> Lang {
    if let Some(l) = std::env::var("CLEANMANAGER_LANG").ok().and_then(|v| Lang::parse(&v)) {
        return l;
    }
    system()
}

#[cfg(windows)]
fn system() -> Lang {
    // SAFETY: no arguments; returns the user's UI language identifier.
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    // Primary language ID 0x04 is Chinese; only Simplified (PRC, Singapore) maps to zh-CN.
    if id & 0x3ff == 0x04 && matches!(id >> 10, 0x02 | 0x04) { Lang::ZhCn } else { Lang::En }
}

#[cfg(not(windows))]
fn system() -> Lang {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .map(|v| if v.to_ascii_lowercase().starts_with("zh_cn") || v.to_ascii_lowercase().starts_with("zh-hans") { Lang::ZhCn } else { Lang::En })
        .unwrap_or(Lang::En)
}

static CURRENT: OnceLock<Lang> = OnceLock::new();

pub fn set(lang: Lang) {
    let _ = CURRENT.set(lang);
}

pub fn current() -> Lang {
    *CURRENT.get_or_init(detect)
}

/// (key, English, 简体中文). Keys are stable identifiers, never shown to users.
const STRINGS: &[(&str, &str, &str)] = &[
    ("kind.capabilities", "Capabilities", "能力"),
    ("kind.scan_result", "Scan result", "扫描结果"),
    ("kind.scan_status", "Scan status (still running)", "扫描状态（仍在进行）"),
    ("kind.scan_cancel_receipt", "Scan cancel receipt (not a terminal state)", "扫描取消回执（不是终态）"),
    ("kind.progress", "Progress", "进度"),
    ("kind.entries_page", "Entries", "目录项"),
    ("kind.analysis_result", "Analysis result", "分析结果"),
    ("kind.candidates_page", "Candidates", "候选"),
    ("kind.plan", "Plan", "计划"),
    ("kind.authorization_status", "Authorization", "授权状态"),
    ("kind.operation_status", "Operation status", "操作状态"),
    ("kind.operation_report", "Operation report", "操作报告"),
    ("kind.operation_list", "Operation history", "操作历史"),
    ("kind.quarantine_list", "Quarantine", "隔离区"),
    ("kind.selection_preview", "Selection preview (dry run, nothing changed)", "选择预览（试运行，未做任何修改）"),
    ("kind.cleanup_catalog", "Cleanup catalog", "清理目录"),
    ("kind.protection_list", "Protection list", "保护名单"),
    ("kind.protection_check", "Protection check", "保护判定"),
    ("kind.app_list", "Applications", "应用"),
    ("kind.startup_list", "Startup items", "启动项"),
    ("kind.app_uninstall_list", "Uninstall routes", "卸载方式"),
    ("kind.uninstall_leftovers", "Uninstall leftovers (grouped)", "卸载残留（分组）"),
    ("kind.status_snapshot", "System status", "系统状态"),
    ("kind.insight_list", "Space insights", "空间洞察"),
    ("kind.insight_overview", "Space insight cards", "空间洞察卡片"),
    ("kind.old_downloads_review", "Old downloads (review only, nothing selected)", "旧下载（仅审阅，无预选）"),
    ("kind.process_list", "Processes", "进程"),
    ("kind.process_pins", "Pinned processes", "置顶进程"),
    ("kind.status_alert", "Sustained high CPU", "持续高 CPU"),
    ("kind.status_alert_settings", "High-CPU alert settings", "高 CPU 提醒设置"),
    ("kind.app_update_list", "App updates", "应用更新"),
    ("kind.app_update_settings", "App update settings", "应用更新设置"),
    ("kind.experimental_features", "Experimental features", "实验功能"),
    ("kind.app_update_plan", "App update plan", "应用更新计划"),
    ("kind.purge_roots", "Project roots", "项目根"),
    ("kind.installer_locations", "Installer locations", "安装包位置"),
    ("kind.doctor_report", "Doctor report", "诊断报告"),
    ("kind.completion_script", "Shell completion", "Shell 补全"),
    ("kind.release_info", "CleanManager release check", "CleanManager 版本检查"),
    ("kind.self_update_result", "CleanManager self-update", "CleanManager 自更新"),
    ("kind.self_install_result", "CleanManager install", "CleanManager 安装"),
    ("kind.self_remove_preview", "CleanManager removal plan", "CleanManager 移除计划"),
    ("kind.error", "Error", "错误"),
    ("code.invalid_argument", "invalid argument or protocol error", "参数或协议错误"),
    ("code.schema_mismatch", "unsupported schema or field", "不支持的协议版本或字段"),
    ("code.needs_approval", "needs your approval: review the plan first", "需要你的批准：请先审阅计划"),
    ("code.policy_denied", "refused by policy or protection", "被策略或保护拒绝"),
    ("code.permission_required", "the system denied access", "系统权限不足"),
    ("code.plan_stale", "the plan or its observations are out of date", "计划或其观察已过期"),
    ("code.cursor_stale", "the page cursor is out of date", "分页游标已过期"),
    ("code.conflict", "conflicts with another plan or operation", "与其他计划或操作冲突"),
    ("code.unsupported", "not supported on this platform or build", "此平台或版本不支持"),
    ("code.io_error", "input/output failure", "读写失败"),
    ("code.internal_error", "internal failure; reconcile before retrying", "内部失败；重试前请先核对"),
    ("code.cancelled", "cancelled", "已取消"),
    ("code.protection_unavailable", "the protection list is unavailable; writes are stopped", "保护名单不可用；已停止写操作"),
    ("code.index_unavailable", "the scan index is unusable (history and recovery are not affected); repair it, then scan again",
        "扫描索引不可用（操作历史与恢复不受影响）；修复后需重新扫描"),
    ("label.detail", "Detail", "详情"),
    ("label.required_action", "Required action", "需要的操作"),
    ("label.items", "items", "项"),
    ("label.started_utc", "started (UTC)", "启动时间（UTC）"),
    ("label.memory", "memory", "内存"),
    ("label.signature", "signature", "签名"),
    ("label.unidentified", "unidentified", "未识别"),
    ("label.unknown", "unknown", "未知"),
    ("label.shown", "shown", "显示"),
    ("label.more", "more", "更多"),
    ("label.machine_hint", "Machine-readable output: add --json (fields and codes are the same in every language).", "机器可读输出：加 --json（字段与代码不随语言变化）。"),
    ("help.title", "CleanManager command line", "CleanManager 命令行"),
    ("help.usage", "Usage: cleanmanager COMMAND [options]   Global: --json | --format json|jsonl|text, --lang zh-CN|en, --non-interactive, --debug (trace to stderr)",
        "用法：cleanmanager 命令 [选项]   通用：--json | --format json|jsonl|text、--lang zh-CN|en、--non-interactive、--debug（轨迹写到标准错误）"),
    ("help.writes", "* creates plans or changes CleanManager's own settings; nothing touches your files without an approved plan.",
        "* 会创建计划或修改 CleanManager 自身设置；没有已批准的计划，任何命令都不会改动你的文件。"),
    ("prompt.completion", "Type 'append' to add these lines to the profile, anything else to cancel: ", "输入 append 把以上内容追加到配置文件，输入其他内容取消："),
    ("prompt.doctor", "Type 'export' to write this redacted report to the file, anything else to cancel: ", "输入 export 把以上脱敏报告写入文件，输入其他内容取消："),
    ("prompt.self_install", "Type 'install' to copy this release into the directory above, anything else to cancel: ", "输入 install 把本发行版复制到上述目录，输入其他内容取消："),
    ("prompt.self_uninstall", "Type 'uninstall' to remove the items above (kept data stays), anything else to cancel: ", "输入 uninstall 移除上述项目（保留的数据不动），输入其他内容取消："),
    ("preview.completion", "Preview: these lines would be appended to", "预览：以下内容将追加到"),
    ("preview.doctor", "Preview of the report that would be written to", "预览：将写入以下文件的报告"),
];

/// Localized text for a key; unknown keys fall back to the key itself (visible, never silent).
pub fn t(key: &str) -> &str {
    match STRINGS.iter().find(|(k, _, _)| *k == key) {
        Some((_, en, zh)) => match current() {
            Lang::En => en,
            Lang::ZhCn => zh,
        },
        None => key,
    }
}

pub fn has(key: &str) -> bool {
    STRINGS.iter().any(|(k, _, _)| *k == key)
}

fn bytes_human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {} ({n} B)", UNITS[u]) }
}

fn scalar(key: &str, v: &Value) -> Option<String> {
    match v {
        Value::Null => Some("null".into()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if key.ends_with("_bytes") => Some(s.parse::<u64>().map(bytes_human).unwrap_or_else(|_| s.clone())),
        Value::String(s) => Some(s.clone()),
        Value::Object(o) if o.contains_key("display") && o.contains_key("raw_base64") => o["display"].as_str().map(str::to_owned),
        _ => None,
    }
}

/// One compact line for an object inside a list: identifiers, states and sizes first.
fn line(v: &Value) -> String {
    let Some(o) = v.as_object() else { return scalar("", v).unwrap_or_else(|| v.to_string()) };
    let mut parts = Vec::new();
    for (k, x) in o {
        if parts.len() >= 6 {
            break;
        }
        if let Some(s) = scalar(k, x).filter(|s| s != "null" && s.chars().count() <= 120) {
            parts.push(format!("{k}={s}"));
        }
    }
    parts.join("  ")
}

/// Render one machine record as human-readable text in the current language. The record's
/// values (codes, states, IDs) are shown verbatim; only titles and explanations are localized.
pub fn render(record: &Value) -> String {
    let kind = record["kind"].as_str().unwrap_or("record");
    let mut out = String::new();
    let title_key = format!("kind.{kind}");
    let title = if has(&title_key) { t(&title_key).to_owned() } else { kind.to_owned() };
    if kind == "error" {
        let code = record["code"].as_str().unwrap_or("internal_error");
        out.push_str(&format!("{title} [{code}]: {}\n", t(&format!("code.{code}"))));
        out.push_str(&format!("  {}: {}\n", t("label.detail"), record["message"].as_str().unwrap_or("")));
        if let Some(a) = record["required_action"].as_str() {
            out.push_str(&format!("  {}: {a}\n", t("label.required_action")));
        }
        for (k, v) in record["details"].as_object().into_iter().flatten() {
            if !v.is_null() {
                out.push_str(&format!("  {k}: {}\n", scalar(k, v).unwrap_or_else(|| v.to_string())));
            }
        }
        return out;
    }
    out.push_str(&format!("{title} [{kind}]\n"));
    let Some(fields) = record.as_object() else { return out };
    for (k, v) in fields {
        if matches!(k.as_str(), "schema_version" | "kind" | "request_id") {
            continue;
        }
        if let Some(s) = scalar(k, v) {
            if k == "script" {
                out.push_str(&format!("  {k}:\n{s}\n"));
            } else {
                out.push_str(&format!("  {k}: {s}\n"));
            }
            continue;
        }
        match v {
            Value::Array(items) => {
                out.push_str(&format!("  {k}: {} {}\n", items.len(), t("label.items")));
                for i in items.iter().take(20) {
                    out.push_str(&format!("    - {}\n", line(i)));
                }
                if items.len() > 20 {
                    out.push_str(&format!("    ... {} {}\n", items.len() - 20, t("label.more")));
                }
            }
            other => out.push_str(&format!("  {k}: {}\n", line(other))),
        }
    }
    out.push_str(&format!("  ({})\n", t("label.machine_hint")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_both_languages_and_no_machine_field_is_a_key() {
        for (k, en, zh) in STRINGS {
            assert!(!en.is_empty() && !zh.is_empty(), "{k}");
            assert!(k.contains('.'), "{k}");
        }
        for code in cm_core::error::ErrorCode::ALL {
            let name = serde_json::to_value(code).unwrap();
            assert!(has(&format!("code.{}", name.as_str().unwrap())), "{name}");
        }
    }

    #[test]
    fn language_tags_parse() {
        assert_eq!(Lang::parse("zh-CN"), Some(Lang::ZhCn));
        assert_eq!(Lang::parse("zh_cn"), Some(Lang::ZhCn));
        assert_eq!(Lang::parse("EN"), Some(Lang::En));
        assert_eq!(Lang::parse("fr"), None);
    }

    #[test]
    fn render_shows_codes_verbatim() {
        let rec = cm_core::protocol::error_record("r", cm_core::error::ErrorCode::NeedsApproval, "plan p1 needs approval", None, Default::default());
        let text = render(&rec);
        assert!(text.contains("[needs_approval]"));
        assert!(text.contains("plan p1 needs approval"));
    }
}
