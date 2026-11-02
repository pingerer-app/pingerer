use super::maintenance::parse_duration;
use super::{duration_of, last, row, Cli, EXIT_DOWN, EXIT_OK};
use crate::client::escape;
use crate::errors::{failed, usage, Error, Result};
use crate::options::value;
use crate::values::{at, object, present, s, truthy};
use serde_json::{json, Value};
use std::time::Instant;

/// The kinds a site's smoke check runs: its uptime monitors.
const AVAILABILITY_KINDS: [&str; 4] = ["http", "tcp", "ping", "smtp"];

impl Cli {
    // check <site-or-monitor>... [--region REGION]
    pub(super) fn command_check(&mut self, args: Vec<String>) -> Result<i32> {
        let (targets, hits) = self.parse(&args, "check <site-or-monitor>... [--region REGION]", vec![value("--region", "REGION", "Check from this region")])?;
        let monitors = self.check_targets(&targets)?;
        let results = self.run_checks(&monitors, last(&hits, "--region"))?;
        if self.output.json {
            self.output.json_value(&Value::Array(results.clone()));
        } else {
            self.print_check_results(&results);
        }
        Ok(if results.iter().all(|it| truthy(at(it, "ok"))) { EXIT_OK } else { EXIT_DOWN })
    }

    // wait-until-up <site-or-monitor>... [--timeout 5m] [--interval 15s] [--region REGION]
    pub(super) fn command_wait_until_up(&mut self, args: Vec<String>) -> Result<i32> {
        let (targets, hits) = self.parse(&args, "wait-until-up <site-or-monitor>... [--timeout 5m] [--interval 15s] [--region REGION]", vec![
            value("--timeout", "DURATION", "How long to wait, like 5m (default 5m)"),
            value("--interval", "DURATION", "How long between checks (default 15s)"),
            value("--region", "REGION", "Check from this region"),
        ])?;
        let timeout = wait_seconds(&last(&hits, "--timeout").unwrap_or_else(|| "5m".into()))?;
        let interval = wait_seconds(&last(&hits, "--interval").unwrap_or_else(|| "15s".into()))?;
        let region = last(&hits, "--region");
        let monitors = self.check_targets(&targets)?;
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            attempt += 1;
            let results = self.run_checks(&monitors, region.clone())?;
            let elapsed = started.elapsed().as_secs_f64();
            let up = results.iter().all(|it| truthy(at(it, "ok")));
            if up || elapsed + interval as f64 > timeout as f64 {
                if self.output.json {
                    self.output.json_value(&Value::Array(results));
                } else {
                    let checks = if attempt == 1 { "check" } else { "checks" };
                    let line = if up { format!("Up after {attempt} {checks} ({}s).", elapsed.round()) } else { format!("Still down after {}:", duration_of(timeout)) };
                    self.output.puts(line);
                    self.print_check_results(&results);
                }
                return Ok(if up { EXIT_OK } else { EXIT_DOWN });
            }
            if !self.output.json {
                let down: Vec<String> = results.iter().filter(|it| !truthy(at(it, "ok"))).map(|it| s(at(it, "name"))).collect();
                self.output.warn(format!("Not up yet ({}); checking again in {interval}s.", down.join(", ")));
            }
            self.sleep(interval as f64);
        }
    }

    /// Monitors (mon_...) as given, and each site's active uptime monitors.
    fn check_targets(&mut self, targets: &[String]) -> Result<Vec<Value>> {
        if targets.is_empty() {
            return usage("What should be checked? Give sites (ids or hosts) or monitor ids.");
        }
        let mut monitors: Vec<Value> = vec![];
        for target in targets {
            let found = if target.starts_with("mon_") {
                vec![at(&self.client()?.get(&format!("/api/v1/monitors/{}", escape(target)), &[])?, "data").clone()]
            } else {
                let uptime: Vec<Value> = self.client()?.get_all("/api/v1/monitors", &[("site_id", Some(target.clone()))])?
                    .into_iter().filter(|it| AVAILABILITY_KINDS.contains(&s(at(it, "kind")).as_str())).collect();
                if uptime.is_empty() {
                    return failed(format!("{target} has no uptime monitors (http, tcp or ping) to check."));
                }
                let active: Vec<Value> = uptime.into_iter().filter(|it| !truthy(at(it, "paused"))).collect();
                if active.is_empty() {
                    return failed(format!("{target}'s uptime monitors are paused. Resume them first."));
                }
                active
            };
            for monitor in found {
                if !monitors.iter().any(|it| at(it, "id") == at(&monitor, "id")) {
                    monitors.push(monitor);
                }
            }
        }
        Ok(monitors)
    }

    /// One test run of each monitor: [{"id", "name", "ok", ...}].
    fn run_checks(&mut self, monitors: &[Value], region: Option<String>) -> Result<Vec<Value>> {
        let mut results = vec![];
        for monitor in monitors {
            let id = s(at(monitor, "id"));
            let body = object(vec![("region", region.clone().map(Value::String).unwrap_or(Value::Null))]);
            let mut result = match self.client()?.post(&format!("/api/v1/monitors/{}/test", escape(&id)), body) {
                Ok(response) => at(&response, "data").clone(),
                Err(Error::Api(error)) if error.status == 501 => {
                    // No test runs on this server: go by the monitor's status.
                    let status = s(at(at(&self.client()?.get(&format!("/api/v1/monitors/{}", escape(&id)), &[])?, "data"), "status"));
                    json!({"ok": status == "up", "error": if status == "up" { Value::Null } else { json!(format!("status is {status}")) }})
                }
                Err(error) => return Err(error),
            };
            if let Some(map) = result.as_object_mut() {
                map.insert("id".into(), at(monitor, "id").clone());
                map.insert("name".into(), at(monitor, "name").clone());
                map.insert("site_id".into(), at(monitor, "site_id").clone());
            }
            results.push(result);
        }
        Ok(results)
    }

    fn print_check_results(&mut self, results: &[Value]) {
        let rows = results.iter().map(|result| row![
            s(at(result, "name")), self.output.status(if truthy(at(result, "ok")) { "ok" } else { "failed" }), s(at(result, "status_code")),
            present(at(result, "latency_ms")).map(|ms| format!("{} ms", s(ms))).unwrap_or_default(), s(at(result, "region")), s(at(result, "error")),
            self.output.muted(s(at(result, "id")))
        ]).collect();
        self.output.table(&["MONITOR", "RESULT", "CODE", "TIME", "REGION", "ERROR", "ID"], rows, "Nothing to show.");
    }
}

/// "30s", "5m", "1h" or seconds.
pub(super) fn wait_seconds(given: &str) -> Result<i64> {
    parse_duration(given, 1, &[('s', 1), ('m', 60), ('h', 3600)]).ok_or(()).or_else(|_| usage(format!("{given} isn't a duration like 30s or 5m")))
}
