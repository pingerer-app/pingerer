//! `pingerer github-action`: what the Pingerer GitHub Action
//! (github-action/action.yml) runs. It pauses and resumes sites and
//! monitors, opens and ends maintenance windows, runs checks now, and waits
//! until sites are up. Not listed in `pingerer help`: it reads its inputs
//! from the environment, as action.yml sets them:
//!
//!   PINGERER_API_TOKEN, PINGERER_ACTION, PINGERER_SITES, PINGERER_MONITORS,
//!   PINGERER_DURATION, PINGERER_TITLE, PINGERER_MESSAGE,
//!   PINGERER_MAINTENANCE_WINDOW_ID, PINGERER_TIMEOUT, PINGERER_INTERVAL,
//!   PINGERER_REGION, PINGERER_ACCOUNT, PINGERER_BASE_URL
//!
//! and writes its outputs to $GITHUB_OUTPUT when it's set.

use super::Cli;
use crate::client::{escape, Client};
use crate::errors::{Error, Result};
use crate::values::{at, list, s, truthy};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::time::Instant;

const AVAILABILITY_KINDS: [&str; 4] = ["http", "tcp", "ping", "smtp"];
const ACTIONS: &str = "pause, resume, maintenance-start, maintenance-end, check-now or wait-until-up";

/// Why the action stopped: shown as a GitHub error annotation.
struct Fail(String);

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail(message)
    }
}

impl Cli {
    pub(super) fn command_github_action(&mut self) -> Result<i32> {
        match self.github_action() {
            Ok(()) => Ok(0),
            Err(Fail(message)) => {
                if self.input("GITHUB_ACTIONS").is_empty() {
                    self.output.warn(format!("Error: {message}"));
                } else {
                    self.output.warn(format!("::error title=Pingerer::{message}"));
                }
                Ok(1)
            }
        }
    }

    fn input(&self, name: &str) -> String {
        self.env.get(name).cloned().unwrap_or_default()
    }

    fn input_or(&self, name: &str, default: &str) -> String {
        Some(self.input(name)).filter(|it| !it.is_empty()).unwrap_or_else(|| default.to_string())
    }

    fn github_action(&mut self) -> std::result::Result<(), Fail> {
        let token = self.input("PINGERER_API_TOKEN");
        if token.is_empty() {
            return Err(Fail("api_token is required: a Pingerer API token with read and write access (from a secret).".into()));
        }
        if !self.input("GITHUB_ACTIONS").is_empty() {
            self.output.puts(format!("::add-mask::{token}"));
        }
        let base_url = self.input_or("PINGERER_BASE_URL", crate::DEFAULT_URL).trim_end_matches('/').to_string();
        let mut client = Client::new(&base_url, Some(token), self.transport.clone());
        client.account = Some(self.input("PINGERER_ACCOUNT")).filter(|it| !it.is_empty());
        let sites = split(&self.input("PINGERER_SITES"));
        let monitors = split(&self.input("PINGERER_MONITORS"));
        let action = self.input("PINGERER_ACTION");

        match action.as_str() {
            "pause" | "resume" => {
                need_targets(&sites, &monitors)?;
                let done = if action == "pause" { "Paused" } else { "Resumed" };
                for site in &sites {
                    let site = self.api(&client, "POST", &format!("/api/v1/sites/{}/{action}", escape(site)), Some(json!({})), &[])?;
                    let site = at(&site, "data");
                    self.output.warn(format!("{done} {} ({}): {}", s(at(site, "host")), s(at(site, "id")), s(at(site, "status"))));
                }
                for monitor in &monitors {
                    let monitor = self.api(&client, "POST", &format!("/api/v1/monitors/{}/{action}", escape(monitor)), Some(json!({})), &[])?;
                    let monitor = at(&monitor, "data");
                    self.output.warn(format!("{done} {} ({}): {}", s(at(monitor, "name")), s(at(monitor, "id")), s(at(monitor, "status"))));
                }
            }
            "maintenance-start" => {
                need_sites(&sites)?;
                let duration = seconds(&self.input_or("PINGERER_DURATION", "15m"))?;
                let mut body = Map::new();
                body.insert("sites".into(), json!(sites));
                body.insert("title".into(), json!(self.input_or("PINGERER_TITLE", "Deploy")));
                body.insert("duration".into(), json!(duration));
                body.insert("notify_subscribers_before".into(), json!(false));
                let message = self.input("PINGERER_MESSAGE");
                if !message.is_empty() {
                    body.insert("message".into(), json!(message));
                }
                let window = self.api(&client, "POST", "/api/v1/maintenance_windows", Some(Value::Object(body)), &[])?;
                let window = at(&window, "data");
                self.output.warn(format!("Maintenance {} ({}) is on until {}: checks keep running, but nothing alerts.", s(at(window, "title")), s(at(window, "id")), s(at(window, "ends_at"))));
                self.action_output("maintenance_window_id", &s(at(window, "id")));
            }
            "maintenance-end" => {
                let given = self.input("PINGERER_MAINTENANCE_WINDOW_ID");
                let ids: Vec<String> = if !given.is_empty() {
                    vec![given]
                } else {
                    need_sites(&sites)?;
                    // The in-progress windows with this title that cover any of the sites.
                    let title = self.input_or("PINGERER_TITLE", "Deploy");
                    let windows = self.api_with(&client, "GET", "/api/v1/maintenance_windows", &[("state", Some("upcoming".into())), ("per_page", Some("100".into()))], None, &[])?;
                    let ids: Vec<String> = list(at(&windows, "data")).iter().filter(|window| {
                        let covered: Vec<String> = list(at(window, "sites")).iter().flat_map(|site| [s(at(site, "id")), s(at(site, "host"))]).collect();
                        s(at(window, "state")) == "in_progress" && s(at(window, "title")) == title
                            && (truthy(at(window, "all_sites")) || sites.iter().any(|site| covered.contains(site)))
                    }).map(|window| s(at(window, "id"))).collect();
                    if ids.is_empty() {
                        self.output.warn(format!("No maintenance window titled \"{title}\" is in progress for those sites."));
                    }
                    ids
                };
                for id in &ids {
                    let window = self.api(&client, "POST", &format!("/api/v1/maintenance_windows/{}/end", escape(id)), Some(json!({})), &[])?;
                    let window = at(&window, "data");
                    self.output.warn(format!("Ended maintenance {} ({}): {}.", s(at(window, "title")), s(at(window, "id")), s(at(window, "state"))));
                }
                self.action_output("maintenance_window_id", &ids.join(","));
            }
            "check-now" => {
                need_targets(&sites, &monitors)?;
                let targets = self.action_targets(&client, &sites, &monitors)?;
                let results = self.action_checks(&client, &targets)?;
                self.report(&results);
                self.action_output("results", &Value::Array(results.clone()).to_string());
                if results.iter().all(|it| truthy(at(it, "ok"))) {
                    self.action_output("status", "up");
                } else {
                    self.action_output("status", "down");
                    return Err(Fail(format!("{} failed its check.", failing(&results))));
                }
            }
            "wait-until-up" => {
                need_targets(&sites, &monitors)?;
                let timeout = seconds(&self.input_or("PINGERER_TIMEOUT", "5m"))?;
                let interval = seconds(&self.input_or("PINGERER_INTERVAL", "15s"))?;
                let targets = self.action_targets(&client, &sites, &monitors)?;
                let started = Instant::now();
                let mut attempt = 0;
                loop {
                    attempt += 1;
                    let results = self.action_checks(&client, &targets)?;
                    if results.iter().all(|it| truthy(at(it, "ok"))) {
                        self.output.warn(format!("Up after {attempt} check{} ({}s):", if attempt == 1 { "" } else { "s" }, started.elapsed().as_secs()));
                        self.report(&results);
                        self.action_output("results", &Value::Array(results).to_string());
                        self.action_output("status", "up");
                        return Ok(());
                    }
                    let elapsed = started.elapsed().as_secs() as i64;
                    if elapsed + interval > timeout {
                        self.report(&results);
                        self.action_output("results", &Value::Array(results.clone()).to_string());
                        self.action_output("status", "down");
                        return Err(Fail(format!("Still down after {timeout}s: {}.", failing(&results))));
                    }
                    self.output.warn(format!("Not up yet ({}); checking again in {interval}s.", failing(&results)));
                    self.sleep(interval as f64);
                }
            }
            "" => return Err(Fail(format!("action is required: {ACTIONS}."))),
            other => return Err(Fail(format!("{other} isn't an action: use {ACTIONS}."))),
        }
        Ok(())
    }

    /// One API call, retrying a rate limit (429) three times; a status in
    /// allow comes back as {"status": ...} instead of failing.
    fn api(&mut self, client: &Client, method: &str, path: &str, body: Option<Value>, allow: &[u16]) -> std::result::Result<Value, Fail> {
        self.api_with(client, method, path, &[], body, allow)
    }

    fn api_with(&mut self, client: &Client, method: &str, path: &str, params: &[(&str, Option<String>)], body: Option<Value>, allow: &[u16]) -> std::result::Result<Value, Fail> {
        for attempt in 1..=4 {
            match client.request(method, path, params, body.clone()) {
                Ok(value) => return Ok(value),
                Err(Error::Api(error)) if error.status == 429 && attempt < 4 => {
                    self.output.warn(format!("Rate limited; waiting {}s.", attempt * 5));
                    self.sleep((attempt * 5) as f64);
                }
                Err(Error::Api(error)) if allow.contains(&error.status) => return Ok(json!({"status": error.status})),
                Err(Error::Api(error)) => return Err(Fail(format!("{method} {path}: {}", error.message))),
                Err(_) => return Err(Fail(format!("Couldn't reach {} ({method} {path}).", client.url))),
            }
        }
        unreachable!("the last attempt returns")
    }

    /// The uptime monitors (http, tcp, ping) to check: those named, and each
    /// named site's active ones; sorted, each once.
    fn action_targets(&mut self, client: &Client, sites: &[String], monitors: &[String]) -> std::result::Result<Vec<String>, Fail> {
        let mut targets: Vec<String> = monitors.to_vec();
        for site in sites {
            let found = list(at(&self.api_with(client, "GET", "/api/v1/monitors", &[("site_id", Some(site.clone())), ("per_page", Some("100".into()))], None, &[])?, "data"));
            let uptime: Vec<&Value> = found.iter().filter(|it| AVAILABILITY_KINDS.contains(&s(at(it, "kind")).as_str())).collect();
            if uptime.is_empty() {
                return Err(Fail(format!("{site} has no uptime monitors (http, tcp or ping) to check.")));
            }
            targets.extend(uptime.iter().filter(|it| !truthy(at(it, "paused"))).map(|it| s(at(it, "id"))));
        }
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            return Err(Fail("Nothing to check: the uptime monitors are paused. Resume them first.".into()));
        }
        Ok(targets)
    }

    /// One check of each monitor now: [{id, ok, ...}].
    fn action_checks(&mut self, client: &Client, targets: &[String]) -> std::result::Result<Vec<Value>, Fail> {
        let region = self.input("PINGERER_REGION");
        let body = if region.is_empty() { json!({}) } else { json!({"region": region}) };
        let mut results = vec![];
        for id in targets {
            let response = self.api(client, "POST", &format!("/api/v1/monitors/{}/test", escape(id)), Some(body.clone()), &[501])?;
            let mut data = if at(&response, "status") == &json!(501) {
                // No test runs on this server: go by the monitor's last status.
                let status = s(at(at(&self.api(client, "GET", &format!("/api/v1/monitors/{}", escape(id)), None, &[])?, "data"), "status"));
                json!({"ok": status == "up", "error": if status == "up" { Value::Null } else { json!(format!("status is {status}")) }, "status_code": null, "latency_ms": null, "region": null})
            } else {
                at(&response, "data").clone()
            };
            if let Some(map) = data.as_object_mut() {
                map.insert("id".into(), json!(id));
            }
            results.push(data);
        }
        Ok(results)
    }

    fn report(&mut self, results: &[Value]) {
        for result in results {
            let mut line = format!("  {} {}", if truthy(at(result, "ok")) { "up  " } else { "DOWN" }, s(at(result, "id")));
            if truthy(at(result, "region")) {
                line += &format!(" from {}", s(at(result, "region")));
            }
            if truthy(at(result, "status_code")) {
                line += &format!(": {}", s(at(result, "status_code")));
            }
            if truthy(at(result, "latency_ms")) {
                line += &format!(" in {}ms", s(at(result, "latency_ms")));
            }
            if truthy(at(result, "error")) {
                line += &format!(" ({})", s(at(result, "error")));
            }
            self.output.warn(line);
        }
    }

    fn action_output(&mut self, name: &str, value: &str) {
        let path = self.input("GITHUB_OUTPUT");
        if !path.is_empty()
            && let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(&path) {
                let _ = write!(file, "{name}<<PINGERER_EOF\n{value}\nPINGERER_EOF\n");
            }
        self.output.puts(format!("{name}={value}"));
    }
}

/// "a.com, b.com" or one per line.
fn split(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace()).filter(|it| !it.is_empty()).map(String::from).collect()
}

fn need_targets(sites: &[String], monitors: &[String]) -> std::result::Result<(), Fail> {
    if sites.is_empty() && monitors.is_empty() {
        return Err(Fail("Which ones? Set site (ids like site_8xKq2, or hosts like yourbakery.com) or monitor (ids like mon_8xKq2).".into()));
    }
    Ok(())
}

fn need_sites(sites: &[String]) -> std::result::Result<(), Fail> {
    if sites.is_empty() {
        return Err(Fail("Which sites? Set site (ids or hosts). Maintenance windows cover sites, not single monitors.".into()));
    }
    Ok(())
}

/// "30s", "15m", "2h", "1d" or seconds, as seconds.
fn seconds(given: &str) -> std::result::Result<i64, Fail> {
    super::maintenance::parse_duration(&given.replace(' ', ""), 1, &[('s', 1), ('m', 60), ('h', 3600), ('d', 86_400)])
        .ok_or_else(|| Fail(format!("{given} isn't a duration like 30s, 15m or 2h.")))
}

fn failing(results: &[Value]) -> String {
    results.iter().filter(|it| !truthy(at(it, "ok"))).map(|it| s(at(it, "id"))).collect::<Vec<_>>().join(", ")
}
