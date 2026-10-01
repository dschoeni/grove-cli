//! Minimal strict argv parsing modelled on `node:util.parseArgs`.

use std::collections::{HashMap, HashSet};

use crate::error::{Result, bail};

/// One accepted option. `value` options take an argument (`--from x` or `--from=x`).
pub struct Opt {
    pub long: &'static str,
    pub short: Option<char>,
    pub value: bool,
}

pub const fn flag(long: &'static str) -> Opt {
    Opt { long, short: None, value: false }
}

pub const fn short_flag(long: &'static str, short: char) -> Opt {
    Opt { long, short: Some(short), value: false }
}

pub const fn value(long: &'static str) -> Opt {
    Opt { long, short: None, value: true }
}

pub const HELP: Opt = short_flag("help", 'h');

#[derive(Default, Debug)]
pub struct Parsed {
    flags: HashSet<&'static str>,
    values: HashMap<&'static str, String>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn flag(&self, long: &str) -> bool {
        self.flags.contains(long)
    }

    pub fn value(&self, long: &str) -> Option<String> {
        self.values.get(long).cloned()
    }
}

/// Split argv at the first standalone `--`. Everything before is parsed by the
/// subcommand; everything after is passed through to the child unchanged.
pub fn split_passthrough(argv: &[String]) -> (Vec<String>, Vec<String>) {
    match argv.iter().position(|a| a == "--") {
        Some(idx) => (argv[..idx].to_vec(), argv[idx + 1..].to_vec()),
        None => (argv.to_vec(), Vec::new()),
    }
}

/// Parse `args` against `opts`. Unknown options are an error; a bare `--`
/// ends option parsing and everything after it becomes a positional.
pub fn parse(args: &[String], opts: &[Opt], allow_positionals: bool) -> Result<Parsed> {
    let mut parsed = Parsed::default();
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if arg == "--" {
            parsed.positionals.extend(iter.by_ref().cloned());
            break;
        }

        if let Some(body) = arg.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            let Some(opt) = opts.iter().find(|o| o.long == name) else {
                bail!("Unknown option '--{name}'");
            };
            if opt.value {
                let v = match inline {
                    Some(v) => v,
                    None => match iter.next() {
                        Some(v) => v.clone(),
                        None => bail!("Option '--{name} <value>' argument missing"),
                    },
                };
                parsed.values.insert(opt.long, v);
            } else {
                if inline.is_some() {
                    bail!("Option '--{name}' does not take an argument");
                }
                parsed.flags.insert(opt.long);
            }
            continue;
        }

        if arg.len() > 1 && arg.starts_with('-') {
            // Short option group, e.g. `-h` or `-hf`. A value option must come last.
            let shorts: Vec<char> = arg[1..].chars().collect();
            for (i, ch) in shorts.iter().enumerate() {
                let Some(opt) = opts.iter().find(|o| o.short == Some(*ch)) else {
                    bail!("Unknown option '-{ch}'");
                };
                if opt.value {
                    let rest: String = shorts[i + 1..].iter().collect();
                    let v = if !rest.is_empty() {
                        rest
                    } else {
                        match iter.next() {
                            Some(v) => v.clone(),
                            None => bail!("Option '-{ch}' argument missing"),
                        }
                    };
                    parsed.values.insert(opt.long, v);
                    break;
                }
                parsed.flags.insert(opt.long);
            }
            continue;
        }

        if !allow_positionals {
            bail!("Unexpected argument '{arg}'. This command does not take positional arguments");
        }
        parsed.positionals.push(arg.clone());
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const OPTS: &[Opt] = &[HELP, value("from"), flag("no-sandbox"), short_flag("force", 'f')];

    #[test]
    fn parses_flags_values_and_positionals() {
        let p = parse(&s(&["feat/x", "--from", "dev", "--no-sandbox", "-f"]), OPTS, true).unwrap();
        assert_eq!(p.positionals, s(&["feat/x"]));
        assert_eq!(p.value("from").as_deref(), Some("dev"));
        assert!(p.flag("no-sandbox") && p.flag("force") && !p.flag("help"));
    }

    #[test]
    fn inline_values_and_double_dash() {
        let p = parse(&s(&["--from=main", "--", "--help"]), OPTS, true).unwrap();
        assert_eq!(p.value("from").as_deref(), Some("main"));
        assert_eq!(p.positionals, s(&["--help"]));
        assert!(!p.flag("help"));
    }

    #[test]
    fn rejects_unknown_and_unexpected() {
        assert!(parse(&s(&["--nope"]), OPTS, true).is_err());
        assert!(parse(&s(&["-x"]), OPTS, true).is_err());
        assert!(parse(&s(&["pos"]), OPTS, false).is_err());
        assert!(parse(&s(&["--from"]), OPTS, true).is_err());
        assert!(parse(&s(&["--no-sandbox=1"]), OPTS, true).is_err());
    }

    #[test]
    fn split_passthrough_at_first_double_dash() {
        let (a, p) = split_passthrough(&s(&["feat/x", "--", "-p", "--", "x"]));
        assert_eq!(a, s(&["feat/x"]));
        assert_eq!(p, s(&["-p", "--", "x"]));
    }
}
