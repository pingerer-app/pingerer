//! Command line options, the way Ruby's OptionParser reads them: options and
//! arguments in any order, `--name value` or `--name=value`, short flags,
//! `--[no-]name`, unambiguous abbreviations and `--` to end options.

use crate::errors::{usage, Result};

#[derive(Clone)]
pub struct Opt {
    pub long: &'static str,
    pub short: Option<char>,
    pub arg: Option<&'static str>,
    pub help: &'static str,
    /// --[no-]name: also accepts --no-name.
    pub negatable: bool,
    /// Another long name for it (--site for --sites).
    pub alias: Option<&'static str>,
}

/// A flag: --open.
pub fn flag(long: &'static str, help: &'static str) -> Opt {
    Opt { long, short: None, arg: None, help, negatable: false, alias: None }
}

/// An option that takes a value: --status STATUS.
pub fn value(long: &'static str, arg: &'static str, help: &'static str) -> Opt {
    Opt { long, short: None, arg: Some(arg), help, negatable: false, alias: None }
}

impl Opt {
    pub fn short(mut self, short: char) -> Self {
        self.short = Some(short);
        self
    }

    pub fn negatable(mut self) -> Self {
        self.negatable = true;
        self
    }

    pub fn alias(mut self, alias: &'static str) -> Self {
        self.alias = Some(alias);
        self
    }
}

/// One option as given: its long name, its value, and whether it was --no-.
#[derive(Debug, Clone)]
pub struct Hit {
    pub name: &'static str,
    pub value: Option<String>,
    pub negated: bool,
}

/// Splits args into options (in the order given) and the arguments left.
pub fn parse(args: &[String], opts: &[Opt]) -> Result<(Vec<String>, Vec<Hit>)> {
    let mut positional = vec![];
    let mut hits = vec![];
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            positional.extend(args[index..].iter().cloned());
            break;
        }
        if let Some(rest) = arg.strip_prefix("--").filter(|it| !it.is_empty()) {
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (rest, None),
            };
            let (opt, negated) = find_long(opts, name, arg)?;
            let value = if opt.arg.is_some() && !negated {
                match inline {
                    Some(value) => Some(value),
                    None if index < args.len() => {
                        index += 1;
                        Some(args[index - 1].clone())
                    }
                    None => return usage(format!("missing argument: --{}", opt.long.trim_start_matches("--"))),
                }
            } else {
                if inline.is_some() {
                    return usage(format!("needless argument: {arg}"));
                }
                None
            };
            hits.push(Hit { name: opt.long, value, negated });
        } else if arg.len() > 1 && arg.starts_with('-') && arg.parse::<f64>().is_err() {
            let shorts: Vec<char> = arg[1..].chars().collect();
            let mut position = 0;
            while position < shorts.len() {
                let letter = shorts[position];
                position += 1;
                let Some(opt) = opts.iter().find(|it| it.short == Some(letter)) else {
                    return usage(format!("invalid option: -{letter}"));
                };
                if opt.arg.is_some() {
                    let attached: String = shorts[position..].iter().collect();
                    let value = if !attached.is_empty() {
                        attached
                    } else if index < args.len() {
                        index += 1;
                        args[index - 1].clone()
                    } else {
                        return usage(format!("missing argument: -{letter}"));
                    };
                    hits.push(Hit { name: opt.long, value: Some(value), negated: false });
                    break;
                }
                hits.push(Hit { name: opt.long, value: None, negated: false });
            }
        } else {
            positional.push(arg.clone());
        }
    }
    Ok((positional, hits))
}

fn find_long<'a>(opts: &'a [Opt], name: &str, given: &str) -> Result<(&'a Opt, bool)> {
    let wanted = format!("--{name}");
    let exact = opts.iter().find(|it| it.long == wanted || it.alias == Some(wanted.as_str()));
    if let Some(opt) = exact {
        return Ok((opt, false));
    }
    if let Some(base) = name.strip_prefix("no-") {
        let positive = format!("--{base}");
        if let Some(opt) = opts.iter().find(|it| it.negatable && it.long == positive) {
            return Ok((opt, true));
        }
    }
    // An unambiguous start of a name, like --ope for --open.
    let candidates: Vec<&Opt> = opts.iter().filter(|it| it.long.starts_with(&wanted)).collect();
    match candidates.as_slice() {
        [only] => Ok((only, false)),
        [] => usage(format!("invalid option: {}", given.split('=').next().unwrap_or(given))),
        _ => usage(format!("ambiguous option: {}", given.split('=').next().unwrap_or(given))),
    }
}

/// The options' help lines, like OptionParser#summarize.
pub fn summary(opts: &[Opt]) -> String {
    let mut lines = String::new();
    for opt in opts {
        let long = if opt.negatable { opt.long.replacen("--", "--[no-]", 1) } else { opt.long.to_string() };
        let long = match opt.alias {
            Some(alias) => format!("{long}, {alias}"),
            None => long,
        };
        let long = match opt.arg {
            Some(arg) => format!("{long} {arg}"),
            None => long,
        };
        let left = match opt.short {
            Some(short) => format!("-{short}, {long}"),
            None => format!("    {long}"),
        };
        if left.chars().count() > 32 {
            lines.push_str(&format!("    {left}\n    {:32} {}\n", "", opt.help));
        } else {
            lines.push_str(&format!("    {left:32} {}\n", opt.help));
        }
    }
    lines
}

/// A whole number for an option that needs one (Ruby's Integer).
pub fn integer(hit: &Hit) -> Result<i64> {
    let text = hit.value.clone().unwrap_or_default();
    text.trim().parse().or_else(|_| usage(format!("invalid argument: {} {text}", hit.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|it| it.to_string()).collect()
    }

    #[test]
    fn options_anywhere_values_and_abbreviations() {
        let opts = [flag("--open", "Only open"), value("--status", "S", "A status"), flag("--yes", "Yes").short('y'), flag("--color", "Color").negatable()];
        let (positional, hits) = parse(&args(&["a", "--status=up", "b", "-y", "--ope", "--no-color", "--", "--status"]), &opts).unwrap();
        assert_eq!(positional, vec!["a", "b", "--status"]);
        let names: Vec<_> = hits.iter().map(|it| (it.name, it.value.clone(), it.negated)).collect();
        assert_eq!(names, vec![("--status", Some("up".into()), false), ("--yes", None, false), ("--open", None, false), ("--color", None, true)]);
        assert!(matches!(parse(&args(&["--bogus"]), &opts), Err(crate::errors::Error::Usage(message)) if message == "invalid option: --bogus"));
        assert!(matches!(parse(&args(&["--status"]), &opts), Err(crate::errors::Error::Usage(message)) if message.starts_with("missing argument")));
    }
}
