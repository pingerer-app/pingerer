use super::{has, last, local_time, no_extra, row, subcommand, Cli, EXIT_OK};
use crate::client::escape;
use crate::errors::{usage, Result};
use crate::options::{flag, value};
use crate::values::{at, list, s, truthy};
use serde_json::{json, Map, Value};

const IMPACTS: [&str; 4] = ["operational", "degraded", "partial_outage", "major_outage"];
const UPDATE_STATES: [&str; 5] = ["investigating", "identified", "monitoring", "resolved", "scheduled"];

impl Cli {
    // status-pages [list|show]
    pub(super) fn command_status_pages(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("status-pages", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, _) = self.parse(&args, "status-pages list", vec![])?;
                no_extra(&positional)?;
                let pages = self.client()?.get_all("/api/v1/status_pages", &[])?;
                if self.json_out(&Value::Array(pages.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = pages.iter().map(|page| row![s(at(page, "name")), s(at(page, "url")), if truthy(at(page, "published")) { "published" } else { "draft" }, self.output.muted(s(at(page, "slug")))]).collect();
                self.output.table(&["NAME", "URL", "STATE", "SLUG"], rows, "No status pages yet. Make one in the app under Status pages.");
            }
            "show" => {
                let id = self.one_arg(&args, "status-pages show <page>")?;
                let page = at(&self.client()?.get(&format!("/api/v1/status_pages/{}", escape(&id)), &[])?, "data").clone();
                if self.json_out(&page) {
                    return Ok(EXIT_OK);
                }
                let fields = vec![
                    ("Page".to_string(), self.output.bold(s(at(&page, "name")))),
                    ("URL".to_string(), s(at(&page, "url"))),
                    ("Status".to_string(), self.impact_text(at(&page, "status"))),
                    ("Published".to_string(), if truthy(at(&page, "published")) { "yes".into() } else { "no (draft)".into() }),
                    ("Subscribers".to_string(), s(at(&page, "subscribers"))),
                    ("ID".to_string(), s(at(&page, "id"))),
                ];
                self.output.fields(fields);
                let components = list(at(&page, "components"));
                if !components.is_empty() {
                    self.output.puts("");
                    let rows = components.iter().map(|component| row![s(at(component, "name")), self.impact_text(at(component, "status"))]).collect();
                    self.output.table(&["COMPONENT", "STATUS"], rows, "Nothing to show.");
                }
                let badges: Vec<(String, String)> = at(&page, "badges").as_object().map(|badges| {
                    badges.iter().filter(|(kind, _)| kind.as_str() != "link").map(|(kind, url)| (capitalize(kind), s(url))).collect()
                }).unwrap_or_default();
                if !badges.is_empty() {
                    self.output.puts("");
                    let title = self.output.bold("Badges");
                    self.output.puts(title);
                    self.output.fields(badges);
                    let link = at(at(&page, "badges"), "link");
                    if truthy(link) {
                        let note = self.output.muted(format!("Each one links to {}.", s(link)));
                        self.output.puts(note);
                    }
                }
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    // updates [list|post]
    pub(super) fn command_updates(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("updates", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, hits) = self.parse(&args, "updates list <page> [--open]", vec![flag("--open", "Only unresolved threads")])?;
                let Some(page) = positional.first().cloned() else { return usage("Which status page? `pingerer updates list <page>`") };
                no_extra(&positional[1..])?;
                let params = [("open", has(&hits, "--open").then(|| "true".to_string())), ("per_page", Some("50".into()))];
                let updates = list(at(&self.client()?.get(&format!("/api/v1/status_pages/{}/updates", escape(&page)), &params)?, "data"));
                if self.json_out(&Value::Array(updates.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = updates.iter().map(|update| row![local_time(at(update, "published_at")), s(at(update, "state")), s(at(update, "title")), s(at(update, "source")), self.output.muted(s(at(update, "id")))]).collect();
                self.output.table(&["POSTED", "STATE", "TITLE", "BY", "ID"], rows, "No updates yet.");
            }
            "post" => {
                let (positional, hits) = self.parse(&args, "updates post <page> --state STATE --message TEXT [--title TITLE] [--component NAME=IMPACT] [--follow-up ID]", vec![
                    value("--state", "STATE", "investigating, identified, monitoring, resolved, scheduled"),
                    value("--message", "TEXT", "What customers should know").short('m'),
                    value("--title", "TITLE", "The headline (not needed with --follow-up)"),
                    value("--component", "NAME=IMPACT", "An affected component and its impact (operational, degraded, partial_outage, major_outage); repeatable"),
                    value("--follow-up", "ID", "Follow up on an earlier update (its id)"),
                ])?;
                let mut components = Map::new();
                for hit in hits.iter().filter(|hit| hit.name == "--component") {
                    let given = hit.value.clone().unwrap_or_default();
                    let (name, impact) = given.split_once('=').unwrap_or((given.as_str(), ""));
                    if name.trim().is_empty() || !IMPACTS.contains(&impact) {
                        return usage("--component takes NAME=IMPACT, like \"Online shop=partial_outage\"");
                    }
                    components.insert(name.trim().to_string(), json!(impact));
                }
                let Some(page) = positional.first().cloned() else { return usage("Which status page? `pingerer updates post <page> --state investigating --message ...`") };
                no_extra(&positional[1..])?;
                let state = last(&hits, "--state").unwrap_or_default();
                if !UPDATE_STATES.contains(&state.as_str()) {
                    return usage(format!("--state is needed ({})", UPDATE_STATES.join(", ")));
                }
                let title = last(&hits, "--title");
                let thread = last(&hits, "--follow-up");
                if title.as_deref().unwrap_or("").trim().is_empty() && thread.is_none() {
                    return usage("--title is needed for a new update (or --follow-up ID)");
                }
                let mut body = Map::new();
                if !components.is_empty() {
                    body.insert("components".into(), Value::Object(components));
                }
                body.insert("state".into(), json!(state));
                if let Some(message) = last(&hits, "--message") {
                    body.insert("message".into(), json!(message));
                }
                if let Some(title) = title {
                    body.insert("title".into(), json!(title));
                }
                if let Some(thread) = thread {
                    body.insert("thread_id".into(), json!(thread));
                }
                let update = at(&self.client()?.post(&format!("/api/v1/status_pages/{}/updates", escape(&page)), Value::Object(body))?, "data").clone();
                if self.json_out(&update) {
                    return Ok(EXIT_OK);
                }
                let line = format!("Posted {} ({}) to {page}. {}", self.output.bold(s(at(&update, "title"))), s(at(&update, "state")), self.output.muted(s(at(&update, "id"))));
                self.output.puts(line);
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    fn impact_text(&self, impact: &Value) -> String {
        let impact = s(impact);
        let word = match impact.as_str() {
            "operational" => "up",
            "degraded" => "degraded",
            "partial_outage" | "major_outage" => "down",
            other => other,
        };
        format!("{} {}", self.output.status(word), impact.replace('_', " ")).trim().to_string()
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}
