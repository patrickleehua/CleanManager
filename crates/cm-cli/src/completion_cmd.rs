//! Shell completion (LIF-03) and `help`, generated from the argument table `args::COMMANDS`.
//!
//! - `completion print SHELL`: the script on stdout (`--format text` prints the bare script for
//!   `eval`/`source`; JSON wraps it in a `completion_script` record).
//! - `completion install SHELL [--profile PATH]`: always shows exactly what would be appended to
//!   which file first. Only a person in an interactive text terminal who types `append` gets the
//!   file written; machine callers (`--json`, non-interactive, no TTY) only ever get the preview.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cm_core::error::ErrorCode;
use cm_core::protocol::record;
use serde_json::{Value, json};

use crate::args::{ARG_VALUES, COMMANDS, GLOBAL_FLAGS, GLOBAL_OPTIONS, HIDDEN_OPTIONS, OPTION_VALUES, PATH_OPTIONS, Parsed};
use crate::commands::fields;
use crate::i18n::{Lang, t};
use crate::{Format, emit, fail};

pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];
const BEGIN: &str = "# >>> cleanmanager completion >>>";
const END: &str = "# <<< cleanmanager completion <<<";

/// What completion offers after a sequence of command words.
pub struct Model {
    /// Space-joined command words → next command words (and closed positional values).
    pub words: BTreeMap<String, Vec<String>>,
    /// Space-joined command words → `--options` and `--flags` of that command (globals included).
    pub options: BTreeMap<String, Vec<String>>,
}

pub fn model() -> Model {
    let mut words: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut options: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for spec in COMMANDS {
        for i in 0..spec.words.len() {
            let next = words.entry(spec.words[..i].join(" ")).or_default();
            if !next.iter().any(|w| w == spec.words[i]) {
                next.push(spec.words[i].to_owned());
            }
        }
        let key = spec.words.join(" ");
        if let Some((_, values)) = spec.arg.and_then(|a| ARG_VALUES.iter().find(|(n, _)| *n == a)) {
            words.entry(key.clone()).or_default().extend(values.iter().map(|v| v.to_string()));
        }
        let opts = options.entry(key).or_default();
        for o in spec.options.iter().chain(GLOBAL_OPTIONS).filter(|o| !HIDDEN_OPTIONS.contains(o)) {
            opts.push(format!("--{o}"));
        }
        for f in spec.flags.iter().chain(GLOBAL_FLAGS) {
            opts.push(format!("--{f}"));
        }
        opts.sort();
        opts.dedup();
    }
    Model { words, options }
}

fn value_options() -> Vec<(String, Vec<&'static str>)> {
    OPTION_VALUES.iter().map(|(o, v)| (format!("--{o}"), v.to_vec())).collect()
}

/// Options whose next argument is a free value (not completed from the command words).
fn valued_options() -> Vec<String> {
    let mut all: Vec<String> =
        COMMANDS.iter().flat_map(|s| s.options.iter()).chain(GLOBAL_OPTIONS).filter(|o| !HIDDEN_OPTIONS.contains(o)).map(|o| format!("--{o}")).collect();
    all.sort();
    all.dedup();
    all
}

fn sh_case(map: &BTreeMap<String, Vec<String>>) -> String {
    map.iter().map(|(k, v)| format!("        \"{k}\") echo \"{}\" ;;\n", v.join(" "))).collect()
}

fn bash() -> String {
    let m = model();
    let values: String = value_options().iter().map(|(o, v)| format!("        {o}) echo \"{}\" ;;\n", v.join(" "))).collect();
    let paths = PATH_OPTIONS.iter().map(|o| format!("--{o}")).collect::<Vec<_>>().join("|");
    format!(
        r#"# cleanmanager bash completion, generated from the CLI argument table (`cleanmanager completion print bash`).
_cleanmanager_words() {{
    case "$1" in
{words}    esac
}}
_cleanmanager_opts() {{
    case "$1" in
{opts}    esac
}}
_cleanmanager_values() {{
    case "$1" in
{values}    esac
}}
_cleanmanager() {{
    local cur prev key i w skip next
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    key=""
    skip=0
    for ((i = 1; i < COMP_CWORD; i++)); do
        w="${{COMP_WORDS[i]}}"
        if [[ $skip == 1 ]]; then skip=0; continue; fi
        case " {valued} " in *" $w "*) skip=1; continue ;; esac
        [[ $w == --* ]] && continue
        next="$(_cleanmanager_words "$key")"
        case " $next " in *" $w "*) key="${{key:+$key }}$w" ;; esac
    done
    case "$prev" in
        {paths}) COMPREPLY=($(compgen -f -- "$cur")); return ;;
    esac
    local vals
    vals="$(_cleanmanager_values "$prev")"
    if [[ -n $vals ]]; then COMPREPLY=($(compgen -W "$vals" -- "$cur")); return; fi
    case " {valued} " in *" $prev "*) COMPREPLY=(); return ;; esac
    if [[ $cur == -* ]]; then
        COMPREPLY=($(compgen -W "$(_cleanmanager_opts "$key")" -- "$cur"))
    else
        COMPREPLY=($(compgen -W "$(_cleanmanager_words "$key")" -- "$cur"))
    fi
}}
complete -o default -F _cleanmanager cleanmanager
"#,
        words = sh_case(&m.words),
        opts = sh_case(&m.options),
        values = values,
        valued = valued_options().join(" "),
        paths = paths,
    )
}

fn zsh() -> String {
    format!(
        "# cleanmanager zsh completion (bash-compatible functions via bashcompinit), generated from the CLI argument table.\nautoload -U +X compinit && compinit\nautoload -U +X bashcompinit && bashcompinit\n{}",
        bash().lines().skip(1).collect::<Vec<_>>().join("\n") + "\n"
    )
}

fn fish() -> String {
    let m = model();
    let mut out = String::from("# cleanmanager fish completion, generated from the CLI argument table (`cleanmanager completion print fish`).\n");
    out.push_str("function __cleanmanager_key\n    set -l words (commandline -opc)\n    set -e words[1]\n    set -l key ''\n    set -l skip 0\n");
    out.push_str(&format!("    set -l valued {}\n", valued_options().join(" ")));
    out.push_str("    for w in $words\n        if test $skip = 1\n            set skip 0\n            continue\n        end\n        if contains -- $w $valued\n            set skip 1\n            continue\n        end\n        if string match -q -- '--*' $w\n            continue\n        end\n        if contains -- $w (__cleanmanager_next \"$key\")\n            set key (string trim -- \"$key $w\")\n        end\n    end\n    echo $key\nend\n");
    out.push_str("function __cleanmanager_next\n    switch \"$argv[1]\"\n");
    for (k, v) in &m.words {
        out.push_str(&format!("        case '{k}'\n            printf '%s\\n' {}\n", v.join(" ")));
    }
    out.push_str("    end\nend\ncomplete -c cleanmanager -f\n");
    for (k, v) in &m.words {
        out.push_str(&format!("complete -c cleanmanager -n 'test (__cleanmanager_key) = \"{k}\"' -a '{}'\n", v.join(" ")));
    }
    let values: BTreeMap<String, Vec<&str>> = value_options().into_iter().collect();
    for (k, opts) in &m.options {
        for o in opts {
            let name = o.trim_start_matches("--");
            let tail = if let Some(v) = values.get(o) {
                format!(" -x -a '{}'", v.join(" "))
            } else if PATH_OPTIONS.contains(&name) {
                " -r -F".to_owned()
            } else if valued_options().contains(o) {
                " -x".to_owned()
            } else {
                String::new()
            };
            out.push_str(&format!("complete -c cleanmanager -n 'test (__cleanmanager_key) = \"{k}\"' -l {name}{tail}\n"));
        }
    }
    out
}

fn ps_table(map: &BTreeMap<String, Vec<String>>) -> String {
    map.iter().map(|(k, v)| format!("    '{k}' = @({})\n", v.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(", "))).collect()
}

fn powershell() -> String {
    let m = model();
    let values: String = value_options().iter().map(|(o, v)| format!("    '{o}' = @({})\n", v.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(", "))).collect();
    let quote = |v: Vec<String>| v.iter().map(|x| format!("'{x}'")).collect::<Vec<_>>().join(", ");
    format!(
        r#"# cleanmanager PowerShell completion, generated from the CLI argument table (`cleanmanager completion print powershell`).
$__cmWords = @{{
{words}}}
$__cmOpts = @{{
{opts}}}
$__cmValues = @{{
{values}}}
$__cmValued = @({valued})
$__cmPaths = @({paths})
Register-ArgumentCompleter -Native -CommandName cleanmanager, cleanmanager.exe -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    $elements = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object {{ $_.ToString() }})
    if ($wordToComplete -ne '' -and $elements.Count -gt 0) {{ $elements = @($elements | Select-Object -SkipLast 1) }}
    $key = ''
    $skip = $false
    foreach ($w in $elements) {{
        if ($skip) {{ $skip = $false; continue }}
        if ($__cmValued -contains $w) {{ $skip = $true; continue }}
        if ($w.StartsWith('--')) {{ continue }}
        $next = $__cmWords[$key]
        if ($next -and ($next -contains $w)) {{ $key = ($key + ' ' + $w).Trim() }}
    }}
    $prev = if ($elements.Count -gt 0) {{ $elements[-1] }} else {{ '' }}
    if ($__cmPaths -contains $prev) {{ return }}
    if ($__cmValues.ContainsKey($prev)) {{ $candidates = $__cmValues[$prev] }}
    elseif ($__cmValued -contains $prev) {{ return }}
    elseif ($wordToComplete.StartsWith('-')) {{ $candidates = $__cmOpts[$key] }}
    else {{ $candidates = $__cmWords[$key] }}
    @($candidates) | Where-Object {{ $_ -and $_.StartsWith($wordToComplete) }} | ForEach-Object {{
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }}
}}
"#,
        words = ps_table(&m.words),
        opts = ps_table(&m.options),
        values = values,
        valued = quote(valued_options()),
        paths = quote(PATH_OPTIONS.iter().map(|o| format!("--{o}")).collect()),
    )
}

pub fn script(shell: &str) -> Option<String> {
    Some(match shell {
        "bash" => bash(),
        "zsh" => zsh(),
        "fish" => fish(),
        "powershell" => powershell(),
        _ => return None,
    })
}

fn path_value(p: &Path) -> Value {
    match cm_platform::paths::raw_path(p.as_os_str()) {
        Ok(r) => serde_json::to_value(r).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

fn completion_record(req: &str, shell: &str, action: &str, script: &str, profile: Option<&Path>, written: bool) -> Value {
    record("completion_script", req, fields(json!({
        "shell": shell, "action": action, "script": script, "profile_path": profile.map(path_value), "written": written,
    })))
}

fn unknown_shell(req: &str, p: &Parsed, shell: &str) -> ExitCode {
    fail(req, ErrorCode::InvalidArgument, &format!("unsupported shell {shell}; use one of {}", SHELLS.join(", ")), p.format)
}

/// `completion print SHELL`.
pub fn print(req: &str, p: &Parsed, shell: &str) -> ExitCode {
    let Some(text) = script(shell) else { return unknown_shell(req, p, shell) };
    match p.format {
        // Bare script, so `eval "$(cleanmanager completion print bash --format text)"` works.
        Format::Text => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
        Format::Json => emit(&completion_record(req, shell, "print", &text, None, false), p.format),
    }
    ExitCode::SUCCESS
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// The shell's usual per-user profile. PowerShell users with a redirected Documents folder pass `--profile $PROFILE`.
fn default_profile(shell: &str) -> Option<PathBuf> {
    let h = home()?;
    Some(match shell {
        "bash" => h.join(".bashrc"),
        "zsh" => h.join(".zshrc"),
        "fish" => h.join(".config").join("fish").join("config.fish"),
        "powershell" if cfg!(windows) => h.join("Documents").join("PowerShell").join("Microsoft.PowerShell_profile.ps1"),
        "powershell" => h.join(".config").join("powershell").join("Microsoft.PowerShell_profile.ps1"),
        _ => return None,
    })
}

/// The block appended to a profile: it loads the script generated by this executable at shell start.
fn profile_block(shell: &str, exe: &Path) -> String {
    let exe = exe.display().to_string();
    let load = match shell {
        "powershell" => format!("& '{}' completion print powershell --format text | Out-String | Invoke-Expression", exe.replace('\'', "''")),
        "fish" => format!("'{}' completion print fish --format text | source", exe.replace('\'', "\\'")),
        _ => format!("eval \"$('{}' completion print {shell} --format text)\"", exe.replace('\'', "'\\''")),
    };
    format!("{BEGIN}\n{load}\n{END}\n")
}

/// `completion install SHELL [--profile PATH]`: preview always; write only after a typed confirmation in a terminal.
pub fn install(req: &str, p: &Parsed, shell: &str) -> ExitCode {
    if script(shell).is_none() {
        return unknown_shell(req, p, shell);
    }
    let Some(profile) = p.raw_option("profile").map(PathBuf::from).or_else(|| default_profile(shell)) else {
        return fail(req, ErrorCode::InvalidArgument, "no home directory; pass --profile PATH", p.format);
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cleanmanager"));
    let block = profile_block(shell, &exe);
    let existing = match std::fs::read_to_string(&profile) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return fail(req, ErrorCode::IoError, &format!("cannot read {}: {e}", profile.display()), p.format),
    };
    if existing.contains(BEGIN) {
        emit(&completion_record(req, shell, "already_installed", &block, Some(&profile), false), p.format);
        return ExitCode::SUCCESS;
    }
    // Machine callers and non-terminals only ever see the preview (LIF-03); nothing is written.
    if p.format == Format::Json || p.non_interactive() {
        emit(&completion_record(req, shell, "preview", &block, Some(&profile), false), p.format);
        return ExitCode::SUCCESS;
    }
    println!("{} {}:\n\n{block}", t("preview.completion"), profile.display());
    match crate::plan_cmd::prompt(t("prompt.completion")) {
        Some(a) if a == "append" => {}
        _ => return fail(req, ErrorCode::Cancelled, "not installed; the profile was not changed", p.format),
    }
    if let Some(dir) = profile.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    let appended = std::fs::OpenOptions::new().create(true).append(true).open(&profile).and_then(|mut f| f.write_all(format!("{sep}{block}").as_bytes()));
    if let Err(e) = appended {
        return fail(req, ErrorCode::IoError, &format!("cannot append to {}: {e}", profile.display()), p.format);
    }
    emit(&completion_record(req, shell, "installed", &block, Some(&profile), true), p.format);
    ExitCode::SUCCESS
}

/// `help`: the command table in the current language (human text only).
pub fn help(req: &str, p: &Parsed) -> ExitCode {
    if p.format == Format::Json {
        return fail(req, ErrorCode::InvalidArgument, "help is human-readable text only (--format text); machine callers read docs/cli-protocol.md or `completion print SHELL --json`", p.format);
    }
    let zh = crate::i18n::current() == Lang::ZhCn;
    let mut out = format!("{}\n{}\n\n", t("help.title"), t("help.usage"));
    for spec in COMMANDS {
        let mut usage = spec.words.join(" ");
        if let Some(a) = spec.arg {
            usage.push_str(&format!(" {a}"));
        }
        let mark = if spec.writes { "*" } else { " " };
        out.push_str(&format!(" {mark}{usage:<34} {}\n", if zh { spec.summary_zh } else { spec.summary_en }));
    }
    out.push_str(&format!("\n{}\n", t("help.writes")));
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_cover_every_command_word_and_option_of_the_table() {
        for shell in SHELLS {
            let s = script(shell).unwrap();
            for spec in COMMANDS {
                for w in spec.words {
                    assert!(s.contains(w), "{shell}: {w}");
                }
                for o in spec.options.iter().filter(|o| !HIDDEN_OPTIONS.contains(o)) {
                    assert!(s.contains(*o), "{shell}: --{o}");
                }
                for f in spec.flags {
                    assert!(s.contains(*f), "{shell}: --{f}");
                }
            }
            for hidden in HIDDEN_OPTIONS {
                assert!(!s.contains(hidden), "{shell} offers internal option {hidden}");
            }
        }
    }

    #[test]
    fn model_nests_subcommands_and_offers_shell_names() {
        let m = model();
        assert!(m.words[""].contains(&"scan".to_string()));
        assert_eq!(m.words["scan"], vec!["show", "wait", "cancel"]);
        assert_eq!(m.words["completion print"], vec!["bash", "zsh", "fish", "powershell"]);
        assert!(m.options["scan wait"].contains(&"--timeout".to_string()));
        assert!(m.options["capabilities"].contains(&"--json".to_string()), "globals everywhere");
        assert!(!m.options["scan"].contains(&"--host-scan-id".to_string()));
    }

    #[test]
    fn profile_block_is_marked_and_loads_this_executable() {
        let b = profile_block("powershell", Path::new("C:\\Tools\\cleanmanager.exe"));
        assert!(b.starts_with(BEGIN) && b.trim_end().ends_with(END));
        assert!(b.contains("'C:\\Tools\\cleanmanager.exe' completion print powershell --format text"));
        assert!(profile_block("bash", Path::new("/usr/local/bin/cleanmanager")).contains("eval \"$('/usr/local/bin/cleanmanager' completion print bash --format text)\""));
    }
}
