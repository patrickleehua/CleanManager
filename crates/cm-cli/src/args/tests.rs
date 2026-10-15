//! The argument table is the single declaration completions and the CLI-12 checks rely on: these
//! tests tie it to the real dispatch in `commands.rs` and check the cross-command invariants.

use super::*;

/// One dispatch arm as written in `commands.rs`: every expansion of its word pattern plus the
/// literal `p.allow(&[options], &[flags])` lists.
#[derive(Debug)]
struct Arm {
    patterns: Vec<(Vec<String>, bool)>,
    options: Vec<String>,
    flags: Vec<String>,
}

fn literals(list: &str) -> Vec<String> {
    list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| s.trim_matches('"').to_owned()).collect()
}

/// Split at `sep` outside brackets.
fn top_level(s: &str, sep: char) -> Vec<String> {
    let (mut depth, mut cur, mut out) = (0i32, String::new(), Vec::new());
    for ch in s.chars() {
        match ch {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
        if ch == sep && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

fn expand(pattern: &str) -> Vec<(Vec<String>, bool)> {
    let pattern = pattern.split(" if ").next().unwrap().trim();
    let mut out = Vec::new();
    for alt in top_level(pattern, '|') {
        let alt = alt.trim();
        let Some(inner) = alt.strip_prefix('[').and_then(|a| a.strip_suffix(']')) else { continue };
        let mut combos: Vec<(Vec<String>, bool)> = vec![(vec![], false)];
        for element in inner.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            // `op @ ("a" | "b")` binds one of several literal words.
            let element = element.split_once(" @ ").map_or(element, |(_, alts)| alts.trim()).trim_start_matches('(').trim_end_matches(')').trim();
            if element.starts_with('"') {
                let choices: Vec<String> = element.split('|').map(|c| c.trim().trim_matches('"').to_owned()).collect();
                combos = combos.into_iter().flat_map(|(w, pos)| choices.iter().map(move |c| ([w.clone(), vec![c.clone()]].concat(), pos))).collect();
            } else {
                combos.iter_mut().for_each(|(_, pos)| *pos = true);
            }
        }
        out.extend(combos);
    }
    out
}

fn dispatch_arms() -> Vec<Arm> {
    let src = include_str!("../commands.rs");
    let start = src.find("let result = match words.as_slice() {").expect("dispatch match");
    let end = start + src[start..].find("\n    match result").expect("end of dispatch");
    let body = &src[start..end];
    let mut arms = Vec::new();
    let mut from = 0;
    while let Some(rel) = body[from..].find(".allow(&[") {
        let at = from + rel;
        let arrow = body[..at].rfind("=>").expect("arm arrow");
        let line_start = body[..arrow].rfind('\n').map_or(0, |i| i + 1);
        let pattern = body[line_start..arrow].trim();
        let rest = &body[at + ".allow(&[".len()..];
        let opts_end = rest.find(']').unwrap();
        let after = &rest[opts_end..];
        let flags_start = after.find("&[").unwrap() + 2;
        let flags_end = after[flags_start..].find(']').unwrap() + flags_start;
        arms.push(Arm { patterns: expand(pattern), options: literals(&rest[..opts_end]), flags: literals(&after[flags_start..flags_end]) });
        from = at + 1;
    }
    arms
}

#[test]
fn every_dispatch_arm_is_declared_in_the_table_and_vice_versa() {
    let arms = dispatch_arms();
    assert!(arms.len() >= 20, "parsed only {} arms", arms.len());
    let mut matched = std::collections::BTreeSet::new();
    for arm in &arms {
        assert!(!arm.patterns.is_empty(), "unparsed arm {arm:?}");
        for (words, positional) in &arm.patterns {
            let spec = COMMANDS
                .iter()
                .find(|s| s.words.iter().copied().eq(words.iter().map(String::as_str)) && s.arg.is_some() == *positional)
                .unwrap_or_else(|| panic!("`{}` is dispatched but missing from args::COMMANDS", words.join(" ")));
            matched.insert(spec.words.join(" "));
            let mut declared: Vec<&str> = spec.options.to_vec();
            let mut accepted: Vec<&str> = arm.options.iter().map(String::as_str).collect();
            declared.sort();
            accepted.sort();
            assert_eq!(declared, accepted, "options of `{}` differ between args::COMMANDS and commands.rs", words.join(" "));
            let mut declared: Vec<&str> = spec.flags.to_vec();
            let mut accepted: Vec<&str> = arm.flags.iter().map(String::as_str).collect();
            declared.sort();
            accepted.sort();
            assert_eq!(declared, accepted, "flags of `{}` differ between args::COMMANDS and commands.rs", words.join(" "));
        }
    }
    for spec in COMMANDS {
        assert!(matched.contains(&spec.words.join(" ")), "`{}` is in args::COMMANDS but not dispatched", spec.words.join(" "));
    }
}

/// CLI-06/07/12 (parity 8.2): no command group has a flag or option that reads as approval.
#[test]
fn no_command_has_a_self_approval_flag() {
    let mut names: Vec<String> = FLAGS.iter().chain(GLOBAL_OPTIONS).chain(GLOBAL_FLAGS).map(|s| s.to_string()).collect();
    for spec in COMMANDS {
        names.extend(spec.options.iter().chain(spec.flags).map(|s| s.to_string()));
    }
    for arm in dispatch_arms() {
        names.extend(arm.options.into_iter().chain(arm.flags));
    }
    for n in &names {
        assert!(!SELF_APPROVAL_NAMES.contains(&n.as_str()), "--{n} would read as self-approval");
        assert!(!n.contains("approve") && !n.starts_with("yes"), "--{n} would read as self-approval");
    }
    // Parsing never turns them into anything either: they are unknown everywhere.
    for spec in COMMANDS {
        for name in SELF_APPROVAL_NAMES {
            let mut raw: Vec<OsString> = spec.words.iter().map(OsString::from).collect();
            if spec.arg.is_some() {
                raw.push("x".into());
            }
            raw.push(format!("--{name}").into());
            raw.push("1".into());
            let p = parse(&raw).unwrap();
            assert!(p.allow(spec.options, spec.flags).unwrap_err().contains(name), "{} --{name}", spec.words.join(" "));
        }
    }
    assert!(parse(&["plan".into(), "apply".into(), "p".into(), "-y".into()]).unwrap_err().contains("-y"));
}

#[test]
fn flag_list_matches_the_table() {
    for f in FLAGS {
        assert!(GLOBAL_FLAGS.contains(f) || COMMANDS.iter().any(|s| s.flags.contains(f)), "--{f} is parsed as a flag but no command takes it");
    }
    for spec in COMMANDS {
        for f in spec.flags {
            assert!(FLAGS.contains(f), "--{f} of `{}` is not in FLAGS (it would swallow the next argument)", spec.words.join(" "));
        }
        for o in spec.options {
            assert!(!FLAGS.contains(o), "--{o} is both an option and a flag");
        }
        for o in spec.widening {
            assert!(spec.options.contains(o), "widening option --{o} is not an option of `{}`", spec.words.join(" "));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for spec in COMMANDS {
        assert!(seen.insert(spec.words.join(" ")), "duplicate table row `{}`", spec.words.join(" "));
    }
}

/// CLI-12 (parity 8.2): every option that widens a selection is refused for non-interactive callers, uniformly.
#[test]
fn widening_options_are_refused_for_non_interactive_callers() {
    let mut checked = 0;
    for spec in COMMANDS.iter().filter(|s| !s.widening.is_empty()) {
        assert!(spec.writes, "only selection-making commands widen");
        for o in spec.widening {
            let mut raw: Vec<OsString> = spec.words.iter().map(OsString::from).collect();
            raw.extend([format!("--{o}").into(), "x".into(), "--non-interactive".into()]);
            let p = parse(&raw).unwrap();
            assert!(p.check_selection_widening().unwrap_err().contains("--candidates"));
            checked += 1;
        }
        // Explicit object IDs are not widening.
        let mut raw: Vec<OsString> = spec.words.iter().map(OsString::from).collect();
        raw.extend(["--candidates".into(), "c1".into(), "--non-interactive".into()]);
        assert!(parse(&raw).unwrap().check_selection_widening().is_ok());
    }
    assert!(checked >= 1);
    // Write commands that select without IDs only ever take default_selected rows: none of them may
    // declare a tier- or category-wide include option.
    for spec in COMMANDS.iter().filter(|s| s.writes) {
        for o in spec.options {
            assert!(!["include-review-only", "include-tiers", "all", "include-recent", "include-sessions"].contains(o), "--{o} on `{}`", spec.words.join(" "));
        }
    }
}

#[test]
fn format_aliases_and_errors() {
    let p = |args: &[&str]| parse(&args.iter().map(OsString::from).collect::<Vec<_>>());
    assert_eq!(p(&["capabilities", "--json"]).unwrap().format, Format::Json);
    let jsonl = p(&["scan", "--format", "jsonl"]).unwrap();
    assert!(jsonl.format == Format::Json && jsonl.stream);
    assert_eq!(p(&["capabilities", "--format", "human"]).unwrap().format, Format::Text);
    assert!(p(&["capabilities", "--json", "--format", "human"]).is_err());
    assert!(p(&["capabilities", "--format", "yaml"]).unwrap_err().contains("yaml"));
    assert!(p(&["capabilities", "--lang", "fr"]).unwrap_err().contains("fr"));
    assert_eq!(p(&["capabilities", "--lang", "zh-CN"]).unwrap().lang, Lang::ZhCn);
    assert!(p(&["scan", "show", "s1", "--store"]).unwrap_err().contains("needs a value"));
    assert!(p(&["scan", "--root", "a", "--root", "b"]).unwrap_err().contains("twice"));
    // UNI-11: `--instance` names one more application each time; nothing else may repeat.
    let batch = p(&["apps", "uninstall", "plan", "create", "--instance", "app_a", "--instance", "app_b"]).unwrap();
    assert_eq!((batch.values("instance"), batch.option("instance")), (vec!["app_a", "app_b"], Some("app_a")));
    assert_eq!(p(&["apps", "uninstall", "plan", "create", "--instance", "app_a"]).unwrap().values("instance"), ["app_a"]);
    assert!(p(&["plan", "show", "p", "--store", "a", "--store", "b"]).unwrap_err().contains("twice"));
}

#[test]
fn spec_lookup_handles_positionals() {
    assert_eq!(spec_for(&["plan", "show", "p_1"]).unwrap().words, &["plan", "show"]);
    assert_eq!(spec_for(&["purge"]).unwrap().words, &["purge"]);
    assert_eq!(spec_for(&["completion", "print", "bash"]).unwrap().arg, Some("SHELL"));
    assert!(spec_for(&["nope"]).is_none());
}

/// Every `cleanmanager ...` example in the protocol doc, README and the bundled Skill must parse and be accepted by the
/// argument table (MVP 12.1/12.4: examples match the real CLI). Lines whose comment says `draft`/`草案` document command
/// groups a build may not implement yet and are skipped.
#[test]
fn documented_command_examples_are_accepted_by_the_table() {
    let docs: [(&str, &str); 5] = [
        ("docs/cli-protocol.md", include_str!("../../../../docs/cli-protocol.md")),
        ("README.md", include_str!("../../../../README.md")),
        ("skills/cleanmanager/SKILL.md", include_str!("../../../../skills/cleanmanager/SKILL.md")),
        ("skills/cleanmanager/references/commands.md", include_str!("../../../../skills/cleanmanager/references/commands.md")),
        ("skills/cleanmanager/references/scenarios.md", include_str!("../../../../skills/cleanmanager/references/scenarios.md")),
    ];
    let mut checked = 0;
    for (name, text) in docs {
        let mut examples: Vec<String> = Vec::new();
        let mut fence = false;
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                fence = !fence;
                continue;
            }
            if fence && line.trim_start().starts_with("cleanmanager ") {
                examples.push(line.trim().to_owned());
            } else if !fence {
                // Inline code spans: `cleanmanager ...`.
                let mut rest = line;
                while let Some(i) = rest.find("`cleanmanager ") {
                    let after = &rest[i + 1..];
                    let Some(end) = after.find('`') else { break };
                    examples.push(after[..end].to_owned());
                    rest = &after[end + 1..];
                }
            }
        }
        for ex in examples {
            let (cmd, comment) = split_comment(&ex);
            if comment.contains("draft") || comment.contains("草案") || cmd.contains('…') {
                continue;
            }
            let tokens: Vec<OsString> = shell_words(cmd).into_iter().skip(1).map(OsString::from).collect();
            let p = parse(&tokens).unwrap_or_else(|e| panic!("{name}: `{cmd}`: {e}"));
            let words: Vec<&str> = p.words.iter().map(String::as_str).collect();
            let spec = spec_for(&words).unwrap_or_else(|| panic!("{name}: `{cmd}` is not a command of args::COMMANDS"));
            p.allow(spec.options, spec.flags).unwrap_or_else(|e| panic!("{name}: `{cmd}`: {e}"));
            let positional = words.len() > spec.words.len();
            assert_eq!(positional, spec.arg.is_some(), "{name}: `{cmd}`: positional argument mismatch");
            checked += 1;
        }
    }
    assert!(checked >= 60, "only {checked} examples found");
}

fn split_comment(line: &str) -> (&str, &str) {
    let mut quoted = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '#' if !quoted && line[..i].ends_with(' ') => return (line[..i].trim_end(), &line[i..]),
            _ => {}
        }
    }
    (line, "")
}

fn shell_words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}
