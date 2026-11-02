use super::checks::wait_seconds;
use super::{duration_of, has, last, local_time, no_extra, Cli, EXIT_DOWN, EXIT_OK};
use crate::client::escape;
use crate::errors::{failed, usage, Error, Result};
use crate::options::{flag, value};
use crate::values::{at, dig, int, list, s, truthy};
use serde_json::{json, Value};
use std::time::Instant;

fn severity_color(severity: &str) -> &'static str {
    match severity {
        "critical" => "red",
        "warning" => "yellow",
        "notice" => "blue",
        "pass" => "green",
        "unknown" => "gray",
        _ => "bold",
    }
}

impl Cli {
    // health <site> [--now] [--all]: a site's health score and what to fix.
    // --now checks it again first and waits for the result. Exits 3 when
    // there's a critical problem, so CI can fail on one.
    pub(super) fn command_health(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, hits) = self.parse(&args, "health <site> [--now] [--all] [--timeout 3m]", vec![
            flag("--now", "Check again now and wait for the result (admins, read and write token)"),
            flag("--all", "List what passed too, not just the problems"),
            value("--timeout", "DURATION", "How long --now waits (default 3m)"),
            value("--interval", "DURATION", "How often --now looks for the result (default 10s)"),
        ])?;
        let Some(site) = positional.first().cloned() else { return usage("Which site? `pingerer health yourbakery.com`") };
        no_extra(&positional[1..])?;
        let path = format!("/api/v1/sites/{}/health", escape(&site));
        let health = if has(&hits, "--now") {
            let timeout = wait_seconds(&last(&hits, "--timeout").unwrap_or_else(|| "3m".into()))?;
            let interval = wait_seconds(&last(&hits, "--interval").unwrap_or_else(|| "10s".into()))?;
            self.check_health_now(&path, timeout, interval)?
        } else {
            at(&self.client()?.get(&path, &[])?, "data").clone()
        };
        let critical = int(dig(&health, &["counts", "critical"])) > 0;
        let code = if critical { EXIT_DOWN } else { EXIT_OK };
        if self.json_out(&health) {
            return Ok(code);
        }
        self.print_health(&site, &health, has(&hits, "--all"));
        Ok(code)
    }

    fn check_health_now(&mut self, path: &str, timeout: i64, interval: i64) -> Result<Value> {
        let started = Instant::now();
        let mut health = match self.client()?.post(path, json!({})) {
            Ok(response) => {
                if !self.output.json {
                    self.output.warn("Checking… this usually takes a minute or two.");
                }
                at(&response, "data").clone()
            }
            // One running already is waited for; one that ran a moment ago is shown.
            Err(Error::Api(error)) if ["already_running", "too_soon"].contains(&error.code.as_str()) => {
                if !self.output.json {
                    self.output.warn(&error.message);
                }
                at(&self.client()?.get(path, &[])?, "data").clone()
            }
            Err(error) => return Err(error),
        };
        while truthy(at(&health, "running")) {
            if started.elapsed().as_secs_f64() + interval as f64 > timeout as f64 {
                return failed(format!("The check is still running after {}. See it later with `pingerer health {}`.", duration_of(timeout), s(at(&health, "site_id"))));
            }
            self.sleep(interval as f64);
            health = at(&self.client()?.get(path, &[])?, "data").clone();
        }
        Ok(health)
    }

    fn print_health(&mut self, site: &str, health: &Value, all: bool) {
        if at(health, "score").is_null() {
            let message = if truthy(at(health, "running")) {
                format!("The first health check of {site} is running. Look again in a minute or two.")
            } else {
                format!("{site} hasn't had a health check yet. Run one with `pingerer health {site} --now`.")
            };
            return self.output.puts(message);
        }
        let change = match at(health, "score_change").as_i64() {
            Some(change) if change != 0 => format!(" ({change:+} since the last check)"),
            _ => String::new(),
        };
        let counts = at(health, "counts");
        let problems = ["critical", "warning", "notice"].iter().map(|it| format!("{} {it}", int(at(counts, it)))).collect::<Vec<_>>().join(" · ");
        let fields = vec![
            ("Site".to_string(), self.output.bold(site)),
            ("Score".to_string(), format!("{}{change}", self.output.bold(format!("{}/100", s(at(health, "score")))))),
            ("Problems".to_string(), problems),
            ("Checked".to_string(), local_time(at(health, "checked_at"))),
        ];
        self.output.fields(fields);
        if truthy(at(health, "running")) {
            let note = self.output.muted("A new check is running.");
            self.output.puts(note);
        }
        if truthy(at(health, "error")) {
            self.output.puts(format!("The last check couldn't finish: {}", s(at(health, "error"))));
        }

        let mut findings: Vec<(Value, String)> = list(at(health, "areas")).iter()
            .flat_map(|area| list(at(area, "findings")).into_iter().map(|finding| (finding, s(at(area, "name")))).collect::<Vec<_>>())
            .collect();
        if !all {
            findings.retain(|(finding, _)| !["pass", "unknown"].contains(&s(at(finding, "severity")).as_str()));
        }
        self.output.puts("");
        if findings.is_empty() {
            let none = self.output.muted("Nothing to fix.");
            return self.output.puts(none);
        }
        for (finding, area) in findings {
            let severity_text = s(at(&finding, "severity"));
            let severity = self.output.paint(format!("{severity_text:8}"), severity_color(&severity_text));
            let line = format!("{severity}  {} {}", self.output.bold(s(at(&finding, "title"))), self.output.muted(format!("({area})")));
            self.output.puts(line);
            let detail = s(at(&finding, "detail"));
            if !detail.trim().is_empty() {
                self.output.puts(format!("    {detail}"));
            }
            let fix = s(at(&finding, "fix"));
            if !fix.trim().is_empty() && severity_text != "pass" {
                self.output.puts(format!("    Fix: {fix}"));
            }
        }
    }
}
