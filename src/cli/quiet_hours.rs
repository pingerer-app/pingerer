use super::{local_time, no_extra, subcommand, Cli, EXIT_OK};
use crate::errors::{usage, Result};
use crate::options::{flag, value};
use crate::values::{at, list, s, truthy};
use serde_json::{json, Map, Value};

const WEEKENDS: [&str; 4] = ["sat_sun", "fri_sat", "fri", "sun"];

impl Cli {
    // quiet-hours [show|set]: hold alerts that can wait (SSL, domain, DNS,
    // email setup, slowness) overnight, and all weekend with --weekends.
    // Downtime and missed heartbeats always go through.
    pub(super) fn command_quiet_hours(&mut self, mut args: Vec<String>) -> Result<i32> {
        let mut sub = subcommand("quiet-hours", &mut args)?;
        if sub == "list" {
            sub = "show".into();
        }
        match sub.as_str() {
            "show" => {
                let (positional, _) = self.parse(&args, "quiet-hours [show]", vec![])?;
                no_extra(&positional)?;
                let quiet = at(&self.client()?.get("/api/v1/quiet_hours", &[])?, "data").clone();
                self.print_quiet_hours(&quiet);
            }
            "set" => {
                let (positional, hits) = self.parse(&args, "quiet-hours set [--on|--off] [--from 22:00] [--to 07:00] [--time-zone ZONE] [--[no-]weekends] [--weekend fri_sat]", vec![
                    flag("--on", "Turn quiet hours on"),
                    flag("--off", "Turn quiet hours off"),
                    value("--from", "TIME", "When they start, like 22:00"),
                    value("--to", "TIME", "When they end, like 07:00"),
                    value("--time-zone", "ZONE", "Like Asia/Kuwait or America/Toronto (\"owner\" for the account owner's)"),
                    flag("--weekends", "Quiet all weekend too").negatable(),
                    value("--weekend", "DAYS", "Your weekend: sat_sun, fri_sat, fri, sun, or auto for your time zone's usual one"),
                ])?;
                no_extra(&positional)?;
                let mut body = Map::new();
                for hit in &hits {
                    let given = hit.value.clone().unwrap_or_default();
                    match hit.name {
                        "--on" => { body.insert("enabled".into(), json!(true)); }
                        "--off" => { body.insert("enabled".into(), json!(false)); }
                        "--from" => { body.insert("starts".into(), json!(given)); }
                        "--to" => { body.insert("ends".into(), json!(given)); }
                        "--time-zone" => { body.insert("time_zone".into(), json!(if given == "owner" { String::new() } else { given })); }
                        "--weekends" => { body.insert("weekends".into(), json!(!hit.negated)); }
                        "--weekend" => {
                            if !WEEKENDS.contains(&given.as_str()) && given != "auto" {
                                return usage(format!("--weekend takes {} or auto", WEEKENDS.join(", ")));
                            }
                            body.insert("weekend".into(), json!(if given == "auto" { String::new() } else { given }));
                        }
                        _ => {}
                    }
                }
                if body.is_empty() {
                    return usage("Nothing to change. Try `pingerer quiet-hours set --on --from 22:00 --to 07:00`.");
                }
                let quiet = at(&self.client()?.patch("/api/v1/quiet_hours", Value::Object(body))?, "data").clone();
                self.print_quiet_hours(&quiet);
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    fn print_quiet_hours(&mut self, quiet: &Value) {
        if self.json_out(quiet) {
            return;
        }
        let days = list(at(quiet, "weekend_days")).iter().map(|day| {
            let day = s(day);
            let mut chars = day.chars();
            chars.next().map(|first| first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()).unwrap_or_default()
        }).collect::<Vec<_>>().join(" and ");
        let from_zone = if at(quiet, "weekend").is_null() { " (from your time zone)" } else { "" };
        let fields = vec![
            ("Quiet hours".to_string(), if truthy(at(quiet, "enabled")) { self.output.bold(s(at(quiet, "summary"))) } else { "off".into() }),
            ("Weekend".to_string(), format!("{days}{from_zone}")),
            ("Quiet now".to_string(), if truthy(at(quiet, "quiet_now")) { format!("yes, until {}", local_time(at(quiet, "quiet_until"))) } else { "no".into() }),
        ];
        self.output.fields(fields);
    }
}
