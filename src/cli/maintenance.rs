use super::{has, last, local_time, no_extra, row, subcommand, Cli, EXIT_OK};
use crate::client::escape;
use crate::errors::{failed, usage, Result};
use crate::options::{self, flag, value};
use crate::values::{at, list, s, truthy};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{json, Map, Value};

impl Cli {
    // maintenance [list|add|start|end|cancel]
    pub(super) fn command_maintenance(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("maintenance", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, hits) = self.parse(&args, "maintenance list [--upcoming | --state upcoming|done|cancelled]", vec![
                    flag("--upcoming", "Only upcoming and in-progress windows"),
                    value("--state", "STATE", "upcoming, done or cancelled"),
                ])?;
                no_extra(&positional)?;
                let mut state = None;
                for hit in &hits {
                    match hit.name {
                        "--upcoming" => state = Some("upcoming".to_string()),
                        "--state" => state = hit.value.clone(),
                        _ => {}
                    }
                }
                let windows = self.client()?.get_all("/api/v1/maintenance_windows", &[("state", state)])?;
                if self.json_out(&Value::Array(windows.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = windows.iter().map(|window| {
                    let occurrence = [at(window, "next_occurrence"), at(window, "last_occurrence")].into_iter().find(|it| truthy(it)).unwrap_or(window);
                    let mut when = format!("{} → {}", local_time(at(occurrence, "starts_at")), local_time(at(occurrence, "ends_at")));
                    if s(at(window, "recurrence")) == "weekly" {
                        when += &format!(" (weekly: {})", list(at(window, "days")).iter().map(s).collect::<Vec<_>>().join(","));
                    }
                    row![s(at(window, "title")), maintenance_state(at(window, "state")), when, maintenance_sites(window), self.output.muted(s(at(window, "id")))]
                }).collect();
                self.output.table(&["TITLE", "STATE", "WHEN", "SITES", "ID"], rows, "No maintenance windows. Add one with `pingerer maintenance add \"Server upgrade\" --start \"2026-10-14 02:00\" --for 30m --all-sites`.");
            }
            "add" => {
                let (positional, hits) = self.parse(&args, "maintenance add <title> --start TIME (--end TIME | --for 30m) (--sites a.com,b.com | --all-sites) [--weekly --days sun,wed]", vec![
                    value("--start", "TIME", "When it starts, like \"2026-10-14 02:00\" (your local time, or --time-zone's)"),
                    value("--end", "TIME", "When it ends"),
                    value("--for", "DURATION", "How long it lasts instead of --end, like 30m or 2h"),
                    value("--sites", "LIST", "Sites affected: ids or hosts, comma separated"),
                    flag("--all-sites", "Every site"),
                    value("--message", "TEXT", "What your status page says"),
                    flag("--weekly", "Repeat every week"),
                    value("--days", "LIST", "With --weekly, the days: sun,mon,..."),
                    value("--time-zone", "ZONE", "Read --start and --end in this zone, like America/Toronto"),
                    value("--notify", "HOURS", "Email status page subscribers this many hours before (default 24)"),
                    flag("--no-notify", "Don't email status page subscribers"),
                ])?;
                let title = positional.join(" ").trim().to_string();
                if title.is_empty() {
                    return usage("Give it a title: `pingerer maintenance add \"Server upgrade\" --start ...`");
                }
                let mut notify: Option<Value> = None;
                for hit in &hits {
                    match hit.name {
                        "--notify" => notify = Some(json!(options::integer(hit)?)),
                        "--no-notify" => notify = Some(json!(false)),
                        _ => {}
                    }
                }
                let start = last(&hits, "--start");
                let end = last(&hits, "--end");
                let length = last(&hits, "--for");
                let sites = last(&hits, "--sites").map(|it| split_list(&it));
                if start.is_none() {
                    return usage("When does it start? Add --start \"2026-10-14 02:00\"");
                }
                if end.is_none() && length.is_none() {
                    return usage("When does it end? Add --end TIME or --for 30m");
                }
                if sites.is_none() && !has(&hits, "--all-sites") {
                    return usage("Which sites? Add --sites a.com,b.com or --all-sites");
                }

                let zone = last(&hits, "--time-zone");
                let mut body = Map::new();
                body.insert("title".into(), json!(title));
                let starts = maintenance_time(start.as_deref().unwrap_or_default(), zone.is_some())?;
                body.insert("starts_at".into(), json!(starts.text));
                let ends = match end {
                    Some(end) => maintenance_time(&end, zone.is_some())?.text,
                    None => {
                        let at = starts.at + chrono::Duration::seconds(maintenance_duration(length.as_deref().unwrap_or_default())?);
                        if zone.is_some() { at.format("%Y-%m-%dT%H:%M:%S").to_string() } else { at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false) }
                    }
                };
                body.insert("ends_at".into(), json!(ends));
                if let Some(zone) = zone {
                    body.insert("time_zone".into(), json!(zone));
                }
                if has(&hits, "--all-sites") {
                    body.insert("all_sites".into(), json!(true));
                }
                if let Some(sites) = sites {
                    body.insert("sites".into(), json!(sites));
                }
                if let Some(message) = last(&hits, "--message") {
                    body.insert("message".into(), json!(message));
                }
                if let Some(notify) = notify {
                    body.insert("notify_subscribers_before".into(), notify);
                }
                let days = last(&hits, "--days").map(|it| it.split(',').map(|day| day.trim().to_string()).collect::<Vec<_>>());
                if has(&hits, "--weekly") {
                    body.insert("recurrence".into(), json!("weekly"));
                    if let Some(days) = days {
                        body.insert("days".into(), json!(days));
                    }
                } else if days.is_some() {
                    return usage("--days goes with --weekly");
                }

                let window = at(&self.client()?.post("/api/v1/maintenance_windows", Value::Object(body))?, "data").clone();
                if self.json_out(&window) {
                    return Ok(EXIT_OK);
                }
                let occurrence = if truthy(at(&window, "next_occurrence")) { at(&window, "next_occurrence").clone() } else { window.clone() };
                let line = format!("Scheduled {} ({}): {} → {}, {}.", self.output.bold(s(at(&window, "title"))), s(at(&window, "id")),
                    local_time(at(&occurrence, "starts_at")), local_time(at(&occurrence, "ends_at")), maintenance_sites(&window));
                self.output.puts(line);
                let note = self.output.muted("Checks keep running, but nothing alerts and the time doesn't count against uptime.");
                self.output.puts(note);
            }
            "start" => {
                let (positional, hits) = self.parse(&args, "maintenance start (--sites a.com,b.com | --site a.com) --for 15m [--title TITLE] [--message TEXT]", vec![
                    value("--sites", "LIST", "Sites: ids or hosts, comma separated (repeatable)").alias("--site"),
                    value("--for", "DURATION", "How long, like 15m or 1h (it can end sooner with `maintenance end`)"),
                    value("--title", "TITLE", "What it's called (default Maintenance)"),
                    value("--message", "TEXT", "What your status page says"),
                    flag("--notify", "Email status page subscribers too"),
                ])?;
                no_extra(&positional)?;
                let sites: Vec<String> = hits.iter().filter(|hit| hit.name == "--sites").flat_map(|hit| split_list(hit.value.as_deref().unwrap_or_default())).collect();
                if sites.is_empty() {
                    return usage("Which sites? Add --sites a.com,b.com");
                }
                let Some(length) = last(&hits, "--for") else { return usage("For how long? Add --for 15m") };
                let mut body = Map::new();
                body.insert("title".into(), json!(last(&hits, "--title").unwrap_or_else(|| "Maintenance".into())));
                body.insert("sites".into(), json!(sites));
                body.insert("duration".into(), json!(maintenance_duration(&length)?));
                if let Some(message) = last(&hits, "--message") {
                    body.insert("message".into(), json!(message));
                }
                if !has(&hits, "--notify") {
                    body.insert("notify_subscribers_before".into(), json!(false));
                }
                let window = at(&self.client()?.post("/api/v1/maintenance_windows", Value::Object(body))?, "data").clone();
                if self.json_out(&window) {
                    return Ok(EXIT_OK);
                }
                let id = s(at(&window, "id"));
                let line = format!("Maintenance {} ({id}) is on until {} for {}.", self.output.bold(s(at(&window, "title"))), local_time(at(&window, "ends_at")), maintenance_sites(&window));
                self.output.puts(line);
                let note = self.output.muted(format!("Checks keep running, but nothing alerts. End it with `pingerer maintenance end {id}`."));
                self.output.puts(note);
            }
            "end" => {
                let (positional, _) = self.parse(&args, "maintenance end <id-or-title>", vec![])?;
                let target = positional.join(" ").trim().to_string();
                if target.is_empty() {
                    return usage("Which window? Give its id or title.");
                }
                let Some(window) = self.find_maintenance(&target)? else { return failed(format!("No maintenance window {target}.")) };
                let window = at(&self.client()?.post(&format!("/api/v1/maintenance_windows/{}/end", escape(&s(at(&window, "id")))), json!({}))?, "data").clone();
                if self.json_out(&window) {
                    return Ok(EXIT_OK);
                }
                self.output.puts(format!("Ended {} ({}).", s(at(&window, "title")), maintenance_state(at(&window, "state"))));
            }
            "cancel" => {
                let (positional, hits) = self.parse(&args, "maintenance cancel <id-or-title> [--yes]", vec![flag("--yes", "Don't ask").short('y')])?;
                let target = positional.join(" ").trim().to_string();
                if target.is_empty() {
                    return usage("Which window? Give its id or title.");
                }
                let Some(window) = self.find_maintenance(&target)? else { return failed(format!("No maintenance window {target}.")) };
                self.confirm(&format!("Cancel the maintenance window {}?", s(at(&window, "title"))), has(&hits, "--yes"))?;
                let window = at(&self.client()?.post(&format!("/api/v1/maintenance_windows/{}/cancel", escape(&s(at(&window, "id")))), json!({}))?, "data").clone();
                if self.json_out(&window) {
                    return Ok(EXIT_OK);
                }
                self.output.puts(format!("Cancelled {}.", s(at(&window, "title"))));
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    fn find_maintenance(&mut self, target: &str) -> Result<Option<Value>> {
        if target.starts_with("mw_") {
            return Ok(Some(at(&self.client()?.get(&format!("/api/v1/maintenance_windows/{}", escape(target)), &[])?, "data").clone()));
        }
        let windows: Vec<Value> = self.client()?.get_all("/api/v1/maintenance_windows", &[("state", Some("upcoming".into()))])?
            .into_iter().filter(|window| s(at(window, "title")).to_lowercase() == target.to_lowercase()).collect();
        Ok(windows.iter().find(|window| s(at(window, "state")) == "in_progress").or(windows.first()).cloned())
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|it| !it.is_empty()).map(String::from).collect()
}

fn maintenance_state(state: &Value) -> String {
    match s(state).as_str() {
        "in_progress" => "in progress".into(),
        "repeats_weekly" => "weekly".into(),
        other => other.into(),
    }
}

fn maintenance_sites(window: &Value) -> String {
    if truthy(at(window, "all_sites")) {
        return "all sites".into();
    }
    list(at(window, "sites")).iter().map(|site| s(at(site, "host"))).collect::<Vec<_>>().join(", ")
}

struct WhenText {
    at: DateTime<FixedOffset>,
    text: String,
}

/// Your local time as ISO 8601 with its offset, or (with --time-zone) the
/// wall clock time for the server to read in that zone.
fn maintenance_time(given: &str, wall_clock: bool) -> Result<WhenText> {
    let Some(at) = parse_time(given.trim()) else { return usage(format!("{given} isn't a time like \"2026-10-14 02:00\"")) };
    let text = if wall_clock { at.format("%Y-%m-%dT%H:%M:%S").to_string() } else { at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false) };
    Ok(WhenText { at, text })
}

/// "2026-10-14 02:00" (this computer's time), or with seconds, a T, an offset or Z.
fn parse_time(text: &str) -> Option<DateTime<FixedOffset>> {
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at);
    }
    for format in ["%Y-%m-%dT%H:%M%:z", "%Y-%m-%d %H:%M%:z", "%Y-%m-%d %H:%M:%S%:z"] {
        if let Ok(at) = DateTime::parse_from_str(text, format) {
            return Some(at);
        }
    }
    let naive = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"].iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .or_else(|| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok().and_then(|date| date.and_hms_opt(0, 0, 0)))?;
    Local.from_local_datetime(&naive).earliest().map(|at| at.fixed_offset())
}

/// "30m", "2h", "1d" or minutes, as seconds.
fn maintenance_duration(given: &str) -> Result<i64> {
    parse_duration(given, 60, &[('s', 1), ('m', 60), ('h', 3600), ('d', 86_400)]).ok_or(()).or_else(|_| usage(format!("{given} isn't a duration like 30m or 2h")))
}

/// A number with an optional unit; plain numbers are in `bare` seconds.
pub(super) fn parse_duration(given: &str, bare: i64, units: &[(char, i64)]) -> Option<i64> {
    let text = given.trim();
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let number: i64 = digits.parse().ok()?;
    let unit = text[digits.len()..].trim_start();
    match unit.chars().collect::<Vec<_>>().as_slice() {
        [] => Some(number * bare),
        [letter] => units.iter().find(|(name, _)| name.eq_ignore_ascii_case(letter)).map(|(_, size)| number * size),
        _ => None,
    }
}
