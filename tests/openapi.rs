//! Every API call the CLI makes is one public/openapi.yaml documents (the
//! same method and path), so the tool and the API can't drift apart.

use regex::Regex;
use std::collections::BTreeSet;
use std::path::Path;

/// "/api/v1/sites/{id}/pause" and "/api/v1/sites/{}/{sub}" both as "/api/v1/sites/{}/...".
fn normalize(path: &str) -> String {
    Regex::new(r"\{[^}]*\}").unwrap().replace_all(path, "{}").to_string()
}

fn documented() -> BTreeSet<(String, String)> {
    let spec = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/openapi.yaml")).expect("public/openapi.yaml");
    let path_line = Regex::new(r"^  (/\S+):\s*$").unwrap();
    let method_line = Regex::new(r"^    (get|post|patch|put|delete):").unwrap();
    let mut found = BTreeSet::new();
    let mut current: Option<String> = None;
    for line in spec.lines() {
        if let Some(path) = path_line.captures(line) {
            current = Some(normalize(&path[1]));
        } else if let (Some(path), Some(method)) = (&current, method_line.captures(line)) {
            found.insert((method[1].to_uppercase(), path.clone()));
        } else if !line.starts_with(' ') && !line.is_empty() {
            current = None;
        }
    }
    found
}

fn called() -> BTreeSet<(String, String)> {
    let call = Regex::new(r#"\.(get_all|get|post|patch|delete)\(\s*&?(?:format!\()?"(/api/v1/[^"]*)""#).unwrap();
    let mut found = BTreeSet::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("cli")];
    let mut sources = vec![];
    while let Some(dir) = files.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.push(path);
            } else if path.extension().is_some_and(|it| it == "rs") {
                sources.push(std::fs::read_to_string(path).unwrap());
            }
        }
    }
    for source in sources {
        for capture in call.captures_iter(&source) {
            let method = if &capture[1] == "get_all" { "GET".to_string() } else { capture[1].to_uppercase() };
            let path = &capture[2];
            // pause and resume are one call: /api/v1/sites/{}/{sub}.
            if path.ends_with("/{sub}") {
                for sub in ["pause", "resume"] {
                    found.insert((method.clone(), normalize(&path.replace("{sub}", sub))));
                }
            } else {
                found.insert((method, normalize(path)));
            }
        }
    }
    found
}

#[test]
fn every_call_is_in_the_openapi_spec() {
    // The spec lives in Pingerer's main repository, next to this folder.
    if !Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/openapi.yaml").exists() {
        eprintln!("No public/openapi.yaml here; skipped (it runs in Pingerer's main repository).");
        return;
    }
    let documented = documented();
    let called = called();
    assert!(called.len() > 30, "found the calls: {called:?}");
    let missing: Vec<_> = called.difference(&documented).collect();
    assert!(missing.is_empty(), "not in public/openapi.yaml: {missing:?}");
}
