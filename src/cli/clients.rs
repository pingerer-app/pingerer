use super::{no_extra, row, subcommand, Cli, EXIT_OK};
use crate::errors::{usage, Result};
use crate::values::{at, s};
use serde_json::{json, Value};

impl Cli {
    // clients [list|add]: an agency's client workspaces. Work inside one
    // with `--account <id>` (or `pingerer use <id>`).
    pub(super) fn command_clients(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("clients", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, _) = self.parse(&args, "clients list", vec![])?;
                no_extra(&positional)?;
                let clients = self.client()?.get_all("/api/v1/clients", &[])?;
                if self.json_out(&Value::Array(clients.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = clients.iter().map(|item| {
                    let uptime = at(item, "uptime_30d").as_f64().map_or("—".to_string(), |percent| format!("{percent:.2}%"));
                    row![s(at(item, "name")), self.output.status(s(at(item, "state"))), s(at(item, "sites")), uptime, s(at(item, "open_incidents")), self.output.muted(s(at(item, "id")))]
                }).collect();
                self.output.table(&["CLIENT", "STATE", "SITES", "UPTIME_30D", "OPEN", "ID"], rows, "No clients yet. Add one with `pingerer clients add <name>`.");
            }
            "add" => {
                let (positional, _) = self.parse(&args, "clients add <name>", vec![])?;
                if positional.is_empty() {
                    return usage("Usage: pingerer clients add <name>");
                }
                let added = at(&self.client()?.post("/api/v1/clients", json!({"name": positional.join(" ")}))?, "data").clone();
                if self.json_out(&added) {
                    return Ok(EXIT_OK);
                }
                let id = s(at(&added, "id"));
                let line = format!("Added {}. {}", self.output.bold(s(at(&added, "name"))), self.output.muted(&id));
                self.output.puts(line);
                self.output.puts(format!("Work in it with `pingerer --account {id} sites add <url>`."));
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }
}
