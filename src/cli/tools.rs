use super::{duration_words, local_time, no_extra, row, Cli, EXIT_DOWN, EXIT_OK};
use crate::client::escape;
use crate::errors::{usage, Result};
use crate::values::{at, int, list, present, s, truthy};
use serde_json::Value;

fn check_color(status: &str) -> &'static str {
    match status {
        "pass" => "green",
        "warn" => "yellow",
        "fail" => "red",
        "info" => "gray",
        _ => "bold",
    }
}

impl Cli {
    // link-preview <page>: how a page looks when its link is shared, and
    // what to fix. Exits 3 when something's broken (a failed check).
    pub(super) fn command_link_preview(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "link-preview <page>", vec![])?;
        let Some(page) = positional.first().cloned() else { return usage("Which page? `pingerer link-preview yourbakery.com/menu`") };
        no_extra(&positional[1..])?;
        let report = at(&self.client()?.get("/api/v1/link_preview", &[("url", Some(page))])?, "data").clone();
        let code = if s(at(&report, "status")) == "fail" { EXIT_DOWN } else { EXIT_OK };
        if self.json_out(&report) {
            return Ok(code);
        }
        if let Some(preview) = present(at(&report, "preview")) {
            let or_dash = |value: &Value, missing: &str| present(value).map(s).unwrap_or_else(|| missing.to_string());
            let fields = vec![
                ("Page".to_string(), self.output.bold(present(at(&report, "url")).or(present(at(&report, "address"))).map(s).unwrap_or_default())),
                ("Title".to_string(), or_dash(at(preview, "title"), "—")),
                ("Description".to_string(), or_dash(at(preview, "description"), "—")),
                ("Picture".to_string(), or_dash(at(preview, "image"), "none")),
                ("Site name".to_string(), or_dash(at(preview, "site_name"), "—")),
            ];
            self.output.fields(fields);
            self.output.puts("");
        }
        for check in list(at(&report, "checks")) {
            let status = s(at(&check, "status"));
            let line = format!("{}  {}", self.output.paint(format!("{status:4}"), check_color(&status)), self.output.bold(s(at(&check, "title"))));
            self.output.puts(line);
            let detail = s(at(&check, "detail"));
            if !detail.trim().is_empty() {
                self.output.puts(format!("    {detail}"));
            }
            let fix = s(at(&check, "fix"));
            if !fix.trim().is_empty() && status != "pass" {
                self.output.puts(format!("    Fix: {fix}"));
            }
        }
        Ok(code)
    }

    // weather [company]: Internet weather, which hosting companies, CDNs and
    // website builders are having trouble now. With a company (its slug,
    // like cloudflare), just that one; exits 3 when it's having trouble.
    pub(super) fn command_weather(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "weather [company]", vec![])?;
        no_extra(positional.get(1..).unwrap_or(&[]))?;
        if let Some(company) = positional.first().cloned() {
            return self.weather_company(&company);
        }

        let weather = at(&self.client()?.get("/api/v1/internet_weather", &[])?, "data").clone();
        if self.json_out(&weather) {
            return Ok(EXIT_OK);
        }
        let companies = list(at(&weather, "companies"));
        let mut troubled: Vec<String> = vec![];
        for company in companies.iter().filter(|it| truthy(at(it, "trouble"))) {
            let name = s(at(company, "name"));
            if !troubled.contains(&name) {
                troubled.push(name);
            }
        }
        let headline = if troubled.is_empty() {
            format!("{} No company is having trouble right now.", self.output.status("ok"))
        } else {
            self.output.paint(format!("Having trouble now: {}", troubled.join(", ")), "red")
        };
        self.output.puts(headline);
        self.output.puts("");
        let rows = companies.iter().map(|listing| row![
            s(at(listing, "name")), s(at(listing, "role")), format!("{}%", s(at(listing, "up_percent"))),
            self.output.status(if truthy(at(listing, "trouble")) { "down" } else { "up" }), self.output.muted(s(at(listing, "slug")))
        ]).collect();
        self.output.table(&["COMPANY", "WHAT", "WORKING", "STATUS", "SLUG"], rows, "We don't watch enough sites on any one company to list it yet.");
        let outages = list(at(&weather, "outages"));
        if !outages.is_empty() {
            self.output.puts("");
            let title = self.output.bold("The past week");
            self.output.puts(title);
            let rows = outages.iter().map(|outage| row![s(at(outage, "name")), local_time(at(outage, "started_at")), outage_length(outage)]).collect();
            self.output.table(&["COMPANY", "STARTED", "HOW LONG"], rows, "Nothing to show.");
        }
        Ok(EXIT_OK)
    }

    fn weather_company(&mut self, slug: &str) -> Result<i32> {
        let company = at(&self.client()?.get(&format!("/api/v1/internet_weather/{}", escape(slug)), &[])?, "data").clone();
        let state = s(at(&company, "state"));
        let code = if state == "trouble" { EXIT_DOWN } else { EXIT_OK };
        if self.json_out(&company) {
            return Ok(code);
        }
        let name = s(at(&company, "name"));
        let verdict = match state.as_str() {
            "trouble" => format!("{} Websites belonging to several customers went down on {name} just now.", self.output.paint("Looks like trouble.", "red")),
            "clear" => format!("{} No problems seen on {name} right now.", self.output.status("ok")),
            "unknown" => format!("We don't watch enough websites on {name} yet to say."),
            other => other.to_string(),
        };
        self.output.puts(verdict);
        let mut fields: Vec<(String, String)> = list(at(&company, "listings")).iter().map(|it| (format!("Working ({})", s(at(it, "role"))), format!("{}%", s(at(it, "up_percent"))))).collect();
        if let Some(page) = present(at(&company, "status_page_url")) {
            fields.push(("Status page".into(), s(page)));
        }
        fields.push(("More".into(), s(at(&company, "url"))));
        self.output.fields(fields);
        let outages = list(at(&company, "outages"));
        if !outages.is_empty() {
            self.output.puts("");
            let title = self.output.bold("Outages in the last 90 days");
            self.output.puts(title);
            let rows = outages.iter().map(|outage| row![local_time(at(outage, "started_at")), outage_length(outage)]).collect();
            self.output.table(&["STARTED", "HOW LONG"], rows, "Nothing to show.");
        }
        Ok(code)
    }
}

fn outage_length(outage: &Value) -> String {
    let length = duration_words(int(at(outage, "duration_seconds")));
    if truthy(at(outage, "ongoing")) { format!("still going ({length} so far)") } else { length }
}
