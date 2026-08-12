use std::fs;
use std::path::Path;

use crate::key::{self, PublicKey};

#[derive(Debug, PartialEq, Eq)]
pub enum Issue {
    Options,
    Wildcard,
    Comma,
    KeyType(String),
    Malformed,
}

#[derive(Debug)]
pub struct Line {
    pub lineno: usize,
    pub name: String,
    pub key: Option<PublicKey>,
    pub issue: Option<Issue>,
}

pub fn load(path: &Path) -> Vec<Line> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(_) => Vec::new(),
    }
}

/// Lenient parse: every non-comment line becomes a Line, carrying either a
/// usable key or the issue that will be refused when the name is used.
/// Bad lines never poison the rest of the file.
pub fn parse(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.split_whitespace();
        let name = f.next().unwrap().to_string();
        let (key, issue) = if name.contains(',') {
            (None, Some(Issue::Comma))
        } else if name.contains('*') || name.contains('?') {
            (None, Some(Issue::Wildcard))
        } else {
            match f.next() {
                None => (None, Some(Issue::Malformed)),
                Some(s) if s.contains('=') => (None, Some(Issue::Options)),
                Some(s) if s == key::ED25519 => match f.next() {
                    Some(b) => match key::parse(&format!("{s} {b}")) {
                        Ok(k) => (Some(k), None),
                        Err(_) => (None, Some(Issue::Malformed)),
                    },
                    None => (None, Some(Issue::Malformed)),
                },
                Some(s) if key::looks_like_key_type(s) => {
                    (None, Some(Issue::KeyType(s.to_string())))
                }
                Some(_) => (None, Some(Issue::Malformed)),
            }
        };
        out.push(Line {
            lineno,
            name,
            key,
            issue,
        });
    }
    out
}

pub fn resolve(lines: &[Line], name: &str, path_pretty: &str) -> Result<PublicKey, String> {
    let matches: Vec<&Line> = lines.iter().filter(|l| l.name == name).collect();
    if matches.is_empty() {
        return Err(format!(
            "no \"{name}\" in {path_pretty}; add: {name} ssh-ed25519 <key>"
        ));
    }
    if matches.len() > 1 {
        let ls: Vec<String> = matches.iter().map(|l| l.lineno.to_string()).collect();
        return Err(format!(
            "\"{name}\" appears {} times in {path_pretty} (lines {}); keep one",
            matches.len(),
            ls.join(", ")
        ));
    }
    let l = matches[0];
    match &l.issue {
        None => Ok(l.key.clone().expect("clean line has a key")),
        Some(Issue::KeyType(t)) => Err(format!(
            "\"{name}\" (line {}) is {t}; beb speaks ssh-ed25519 only",
            l.lineno
        )),
        Some(Issue::Options) => Err(format!(
            "\"{name}\" (line {}) uses options; beb honors plain \"name key\" lines",
            l.lineno
        )),
        Some(Issue::Wildcard) => Err(format!(
            "\"{name}\" (line {}) is a wildcard; beb honors literal names only",
            l.lineno
        )),
        Some(Issue::Comma) => Err(format!(
            "\"{name}\" (line {}) lists several principals; beb honors one name per line",
            l.lineno
        )),
        Some(Issue::Malformed) => Err(format!(
            "\"{name}\" (line {}) is not \"name key\"; fix the line in {path_pretty}",
            l.lineno
        )),
    }
}

/// Display name for a key, from the first clean line that carries it.
pub fn reverse<'a>(lines: &'a [Line], canonical: &str) -> Option<&'a str> {
    lines
        .iter()
        .find(|l| {
            l.issue.is_none() && l.key.as_ref().expect("clean line has a key").canonical() == canonical
        })
        .map(|l| l.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFv7BidWkQPvjU9Qz+J3BWNuFmqssCIorRaHYge3gKOQ";
    const K2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKOBoNSMpcu5CaPKvBT4dO4cH+sHV1Pw0LfkEY1yHOHi";
    const K3: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM6L2tfHal2S7WGgx6K+FDtyk6osS2Xv2KVpm3xAQD8V";
    const K4: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIP1RmQJQRXVH58rvfXe4ajw9O/1oXLlL6lFnYWv6mpFy";

    fn file() -> String {
        format!(
            "# comment\nbackend {K1}\nfrontend {K2}\n\ndup {K3}\ndup {K4}\nlegacy ssh-rsa AAAAB3NzaC1yc2Efakefakefake\nstar* {K1}\nopts namespaces=\"beb\" {K1}\na,b {K1}\nbroken\ngarbage ssh-ed25519 QQ==\n"
        )
    }

    #[test]
    fn resolves_clean_name() {
        let lines = parse(&file());
        let k = resolve(&lines, "backend", "~/ks").unwrap();
        assert_eq!(k.canonical(), K1);
    }

    #[test]
    fn unknown_names_the_line_to_add() {
        let lines = parse(&file());
        let e = resolve(&lines, "nobody", "~/ks").unwrap_err();
        assert!(e.contains("add: nobody ssh-ed25519"), "{e}");
    }

    #[test]
    fn ambiguity_names_both_lines() {
        let lines = parse(&file());
        let e = resolve(&lines, "dup", "~/ks").unwrap_err();
        assert!(e.contains("lines 5, 6"), "{e}");
    }

    #[test]
    fn foreign_type_refused_by_name() {
        let lines = parse(&file());
        let e = resolve(&lines, "legacy", "~/ks").unwrap_err();
        assert!(e.contains("ssh-rsa"), "{e}");
        assert!(e.contains("line 7"), "{e}");
    }

    #[test]
    fn wildcard_options_comma_refused() {
        let lines = parse(&file());
        assert!(resolve(&lines, "star*", "~/ks").unwrap_err().contains("wildcard"));
        assert!(resolve(&lines, "opts", "~/ks").unwrap_err().contains("options"));
        assert!(resolve(&lines, "a,b", "~/ks").unwrap_err().contains("several"));
        assert!(resolve(&lines, "broken", "~/ks").unwrap_err().contains("not \"name key\""));
    }

    #[test]
    fn undecodable_ed25519_line_refused_by_name() {
        let lines = parse(&file());
        let e = resolve(&lines, "garbage", "~/ks").unwrap_err();
        assert!(e.contains("line 12"), "{e}");
    }

    #[test]
    fn bad_lines_do_not_poison() {
        let lines = parse(&file());
        assert!(resolve(&lines, "frontend", "~/ks").is_ok());
    }

    #[test]
    fn reverse_resolves_clean_only() {
        let lines = parse(&file());
        assert_eq!(reverse(&lines, K2), Some("frontend"));
        assert_eq!(reverse(&lines, "ssh-ed25519 AAAAnobody"), None);
    }
}
