//! Strict argument parsing: positional command words plus `--key value` options and flags.
//! Unknown or repeated options are errors; nothing is guessed.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::IsTerminal;

use crate::Format;

/// Options that take no value.
const FLAGS: &[&str] = &["json", "non-interactive", "dry-run"];

#[derive(Debug, Clone)]
pub struct Parsed {
    pub words: Vec<String>,
    pub options: BTreeMap<String, String>,
    /// Option values exactly as received from the OS (paths may not be valid Unicode).
    pub raw_options: BTreeMap<String, OsString>,
    pub flags: Vec<String>,
    pub format: Format,
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

    pub fn flag(&self, key: &str) -> bool {
        self.flags.iter().any(|f| f == key)
    }

    /// Reject options the command does not understand.
    pub fn allow(&self, options: &[&str], flags: &[&str]) -> Result<(), String> {
        for k in self.options.keys() {
            if k != "format" && !options.contains(&k.as_str()) {
                return Err(format!("unknown option --{k}"));
            }
        }
        for f in &self.flags {
            if f != "json" && !flags.contains(&f.as_str()) {
                return Err(format!("unknown option --{f}"));
            }
        }
        Ok(())
    }
}

pub fn parse(raw: &[OsString]) -> Result<Parsed, String> {
    let mut words = Vec::new();
    let mut options = BTreeMap::new();
    let mut raw_options = BTreeMap::new();
    let mut flags = Vec::new();
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
                if options.insert(key.to_owned(), value.to_string_lossy().into_owned()).is_some() {
                    return Err(format!("--{key} given twice"));
                }
                raw_options.insert(key.to_owned(), value.clone());
            }
        } else {
            words.push(arg.to_owned());
        }
    }
    let format = match (flags.iter().any(|f| f == "json"), options.get("format").map(String::as_str)) {
        (true, Some("text")) => return Err("conflicting output formats".into()),
        (true, _) | (false, Some("json")) => Format::Json,
        (false, Some("text")) => Format::Text,
        (false, Some(other)) => return Err(format!("unsupported format: {other}")),
        // Non-TTY stdout defaults to machine output; never start an interactive UI implicitly.
        (false, None) => if std::io::stdout().is_terminal() { Format::Text } else { Format::Json },
    };
    Ok(Parsed { words, options, raw_options, flags, format })
}
