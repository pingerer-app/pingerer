//! `pingerer <command> [subcommand] [args] [options]`. run returns the exit
//! code: 0 ok, 1 an error, 2 wrong usage, 3 something is down (status, and
//! monitors test when the check fails), so CI scripts can act on it.

mod checks;
mod clients;
mod github_action;
mod health;
mod maintenance;
mod quiet_hours;
mod status_pages;
mod tools;

use crate::client::{escape, user_agent, Client, Transport};
use crate::config::Config;
use crate::errors::{failed, usage, Error, Result};
use crate::options::{self, flag, value, Hit, Opt};
use crate::output::Output;
use crate::values::{at, dig, int, list, object, present, s, truthy};
use crate::{DEFAULT_URL, VERSION};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::rc::Rc;

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_DOWN: i32 = 3;

pub const COMMANDS: &[(&str, &str)] = &[
    ("login", "Sign in with an API token"),
    ("logout", "Forget the token"),
    ("whoami", "Who and which account you're using"),
    ("accounts", "List your accounts"),
    ("use", "Use an account from now on: use <account>"),
    ("status", "Everything at a glance (exits 3 when something is down)"),
    ("sites", "list | add <url> | show <site> | pause <site> | resume <site> | rm <site>"),
    ("monitors", "list | show <id> | set <id> [--renews-on DATE] | pause <id> | resume <id> | test <id> | rm <id>"),
    ("incidents", "list [--open] | show <id> | ack <id> | note <id> <text> | resolve <id>"),
    ("heartbeats", "list | add <name> --every 1d --grace 30m [--email] | ping <token-or-name> [--fail] | rm <id>"),
    ("results", "A monitor's recent checks: results <monitor> --since 24h"),
    ("health", "A site's health score and what to fix: health <site> [--now] (exits 3 on a critical problem)"),
    ("maintenance", "list [--upcoming] | add <title> --start TIME --for 30m --all-sites | start --sites a.com --for 15m | end <id-or-title> | cancel <id-or-title>"),
    ("check", "Check sites or monitors once, now: check <site-or-monitor>... (exits 3 when one fails)"),
    ("wait-until-up", "Check until sites or monitors are up: wait-until-up <site-or-monitor>... --timeout 5m (exits 3 if not)"),
    ("status-pages", "list | show <page>"),
    ("updates", "list <page> [--open] | post <page> --state investigating --message TEXT [--title T] [--component NAME=impact] [--follow-up ID]"),
    ("clients", "list | add <name> (agency client workspaces)"),
    ("quiet-hours", "show | set --on --from 22:00 --to 07:00 [--weekends] [--weekend fri_sat]"),
    ("link-preview", "How a page looks when shared: link-preview <page> (exits 3 when something's broken)"),
    ("weather", "Which hosts and CDNs are having trouble: weather [company] (exits 3 when that one is)"),
    ("help", "Show help for a command"),
    ("version", "Show the version"),
];

const SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("sites", &["list", "add", "show", "pause", "resume", "rm"]),
    ("monitors", &["list", "show", "set", "pause", "resume", "test", "rm"]),
    ("incidents", &["list", "show", "ack", "note", "resolve"]),
    ("heartbeats", &["list", "add", "ping", "rm"]),
    ("maintenance", &["list", "add", "start", "end", "cancel"]),
    ("status-pages", &["list", "show"]),
    ("updates", &["list", "post"]),
    ("clients", &["list", "add"]),
    ("quiet-hours", &["show", "set"]),
];

/// Where a run reads and writes, and how it reaches the server; tests pass fakes.
pub struct Io {
    pub out: Box<dyn Write>,
    pub err: Box<dyn Write>,
    pub out_tty: bool,
    pub input: Box<dyn BufRead>,
    pub input_tty: bool,
    pub env: HashMap<String, String>,
    pub transport: Rc<dyn Transport>,
    pub sleeper: Box<dyn FnMut(f64)>,
}

pub fn run(argv: Vec<String>, io: Io) -> i32 {
    Cli::new(argv, io).run()
}

pub struct Cli {
    argv: Vec<String>,
    output: Output,
    input: Box<dyn BufRead>,
    input_tty: bool,
    env: HashMap<String, String>,
    transport: Rc<dyn Transport>,
    sleeper: Box<dyn FnMut(f64)>,
    account_option: Option<String>,
    url_option: Option<String>,
    help: bool,
    version: bool,
    config: Option<Config>,
    client: Option<Rc<Client>>,
}

fn global_opts() -> Vec<Opt> {
    vec![
        flag("--json", "Print JSON instead of tables"),
        value("--account", "ACCOUNT", "Use this account (id or name) for this command"),
        value("--url", "URL", "The Pingerer server (default https://pingerer.com, or PINGERER_URL)"),
        flag("--color", "Color the output (default: on a terminal, unless NO_COLOR is set)").negatable(),
    ]
}

/// Strings for a table row.
macro_rules! row {
    ($($cell:expr),* $(,)?) => { vec![$($cell.to_string()),*] };
}
pub(crate) use row;

impl Cli {
    fn new(argv: Vec<String>, io: Io) -> Self {
        let no_color = io.env.contains_key("NO_COLOR");
        Cli {
            argv,
            output: Output::new(io.out, io.err, io.out_tty, no_color),
            input: io.input,
            input_tty: io.input_tty,
            env: io.env,
            transport: io.transport,
            sleeper: io.sleeper,
            account_option: None,
            url_option: None,
            help: false,
            version: false,
            config: None,
            client: None,
        }
    }

    fn run(&mut self) -> i32 {
        let mut command: Option<String> = None;
        let result = self.dispatch(&mut command);
        match result {
            Ok(code) => code,
            Err(Error::HelpShown) => EXIT_OK,
            Err(Error::Usage(message)) => {
                self.output.warn(format!("pingerer: {message}"));
                let topic = command.filter(|it| it != "help" && known(it)).map(|it| format!(" {it}")).unwrap_or_default();
                self.output.warn(format!("Run `pingerer help{topic}` for usage."));
                EXIT_USAGE
            }
            Err(Error::Api(error)) => {
                self.output.warn(format!("pingerer: {}", error.message));
                if let Some(Value::Object(details)) = &error.details {
                    for (field, messages) in details {
                        for message in list(messages) {
                            let prefix = if field == "base" { String::new() } else { format!("{field}: ") };
                            self.output.warn(format!("  {prefix}{}", s(&message)));
                        }
                    }
                }
                if error.status == 401 {
                    self.output.warn("Run `pingerer login` to sign in again.");
                }
                if error.code == "plan_limit" {
                    self.output.warn("Upgrade your plan, or pause something, to make room.");
                }
                EXIT_ERROR
            }
            Err(Error::Failed(message)) => {
                self.output.warn(format!("pingerer: {message}"));
                EXIT_ERROR
            }
        }
    }

    fn dispatch(&mut self, command_seen: &mut Option<String>) -> Result<i32> {
        let mut args = self.argv.clone();
        // The global options before the command (OptionParser#order!).
        let mut leading = vec![];
        while let Some(first) = args.first() {
            if !first.starts_with('-') || first == "-" || first == "--" {
                break;
            }
            let first = args.remove(0);
            let takes_value = ["--account", "--url"].contains(&first.as_str());
            leading.push(first);
            if takes_value && !args.is_empty() {
                leading.push(args.remove(0));
            }
        }
        let mut opts = global_opts();
        opts.push(flag("--help", "Show help").short('h'));
        opts.push(flag("--version", "Show the version").short('v'));
        let (_, hits) = options::parse(&leading, &opts)?;
        self.apply_global(&hits);

        let mut command = if args.is_empty() { "help".to_string() } else { args.remove(0) };
        if self.help && command != "help" {
            args.insert(0, command);
            command = "help".into();
        }
        if self.version {
            command = "version".into();
        }
        *command_seen = Some(command.clone());
        // What the GitHub Action runs; not listed in help.
        if command == "github-action" {
            return self.command_github_action();
        }
        if !known(&command) {
            return usage(format!("Unknown command: {command}"));
        }

        match command.as_str() {
            "help" => self.command_help(args),
            "version" => self.command_version(),
            "login" => self.command_login(args),
            "logout" => self.command_logout(args),
            "whoami" => self.command_whoami(args),
            "accounts" => self.command_accounts(args),
            "use" => self.command_use(args),
            "status" => self.command_status(args),
            "sites" => self.command_sites(args),
            "monitors" => self.command_monitors(args),
            "incidents" => self.command_incidents(args),
            "heartbeats" => self.command_heartbeats(args),
            "results" => self.command_results(args),
            "health" => self.command_health(args),
            "maintenance" => self.command_maintenance(args),
            "check" => self.command_check(args),
            "wait-until-up" => self.command_wait_until_up(args),
            "status-pages" => self.command_status_pages(args),
            "updates" => self.command_updates(args),
            "clients" => self.command_clients(args),
            "quiet-hours" => self.command_quiet_hours(args),
            "link-preview" => self.command_link_preview(args),
            "weather" => self.command_weather(args),
            _ => usage(format!("Unknown command: {command}")),
        }
    }

    fn apply_global(&mut self, hits: &[Hit]) {
        for hit in hits {
            match hit.name {
                "--json" => self.output.json = true,
                "--account" => self.account_option = hit.value.clone(),
                "--url" => self.url_option = hit.value.clone(),
                "--color" => self.output.color = Some(!hit.negated),
                "--help" => self.help = true,
                "--version" => self.version = true,
                _ => {}
            }
        }
    }

    // --- Commands --------------------------------------------------------------

    fn command_help(&mut self, args: Vec<String>) -> Result<i32> {
        if let Some(topic) = args.first().filter(|it| known(it)) {
            return self.print_command_help(topic);
        }
        self.output.puts(format!("Pingerer {VERSION}: uptime, SSL, DNS and email monitoring from your terminal."));
        self.output.puts("");
        self.output.puts("Usage: pingerer <command> [arguments] [options]");
        self.output.puts("");
        let width = COMMANDS.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
        for (name, text) in COMMANDS {
            self.output.puts(format!("  {name:width$}  {text}"));
        }
        self.output.puts("");
        self.output.puts("Options for every command:");
        let mut opts = global_opts();
        opts.push(flag("--help", "Show help").short('h'));
        opts.push(flag("--version", "Show the version").short('v'));
        self.output.puts(options::summary(&opts).trim_end());
        self.output.puts("");
        self.output.puts("Set PINGERER_TOKEN (and PINGERER_URL) to skip `pingerer login`, as in CI.");
        self.output.puts("Exit codes: 0 ok, 1 error, 2 usage, 3 something is down.");
        Ok(EXIT_OK)
    }

    fn command_version(&mut self) -> Result<i32> {
        self.output.puts(format!("pingerer {VERSION}"));
        Ok(EXIT_OK)
    }

    fn command_login(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, hits) = self.parse(&args, "login [--token TOKEN]", vec![value("--token", "TOKEN", "The API token (otherwise you're asked for it)")])?;
        no_extra(&positional)?;
        let mut token = last(&hits, "--token");
        if token.is_none() {
            token = self.env.get("PINGERER_TOKEN").filter(|it| !it.trim().is_empty()).cloned();
        }
        let token = match token {
            Some(token) => token,
            None => self.prompt_token()?,
        };
        if token.trim().is_empty() {
            return usage("No token given");
        }
        let token = token.trim().to_string();
        let base_url = self.base_url();
        let me = Client::new(&base_url, Some(token.clone()), self.transport.clone()).get("/api/v1/me", &[])?;
        let saved_url = (base_url != DEFAULT_URL).then(|| base_url.clone());
        self.config().update(vec![("token", Some(token)), ("url", saved_url)])?;
        if self.output.json {
            self.output.json_value(&me);
            return Ok(EXIT_OK);
        }
        let name = self.output.bold(s(at(&me, "name")));
        self.output.puts(format!("Logged in to {base_url} as {name}."));
        let path = self.config().path.display().to_string();
        let saved = self.output.muted(format!("Token saved in {path}."));
        self.output.puts(saved);
        Ok(EXIT_OK)
    }

    fn command_logout(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "logout", vec![])?;
        no_extra(&positional)?;
        self.config().update(vec![("token", None), ("account", None)])?;
        let still = if self.config().token_from_env() { " (PINGERER_TOKEN is still set.)" } else { "" };
        self.output.puts(format!("Logged out.{still}"));
        Ok(EXIT_OK)
    }

    fn command_whoami(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "whoami", vec![])?;
        no_extra(&positional)?;
        let me = self.client()?.get("/api/v1/me", &[])?;
        let account = dig(&self.client()?.get("/api/v1/status", &[])?, &["data", "account"]).clone();
        let base_url = self.base_url();
        if self.output.json {
            self.output.json_value(&json!({"user": me, "account": account, "url": base_url}));
            return Ok(EXIT_OK);
        }
        let account_line = present(&account).map(|account| {
            let paused = if truthy(at(account, "suspended")) { " — paused by Pingerer" } else { "" };
            format!("{} ({}){paused}", s(at(account, "name")), s(at(account, "id")))
        });
        let token_line = if self.config().token_from_env() { "from PINGERER_TOKEN".to_string() } else { format!("from {}", self.config().path.display()) };
        self.output.fields(vec![
            ("Name".into(), s(at(&me, "name"))),
            ("Account".into(), account_line.unwrap_or_default()),
            ("Server".into(), base_url),
            ("Token".into(), token_line),
        ]);
        Ok(EXIT_OK)
    }

    fn command_accounts(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "accounts", vec![])?;
        no_extra(&positional)?;
        let accounts = accounts_list(self.client()?.get("/api/v1/accounts", &[])?);
        if self.output.json {
            self.output.json_value(&Value::Array(accounts));
            return Ok(EXIT_OK);
        }
        let current = self.config().account().unwrap_or_default();
        let rows: Vec<Vec<String>> = accounts.iter().map(|account| {
            let mark = if [s(at(account, "id")), s(at(account, "prefix_id"))].contains(&current) { "*" } else { "" };
            row![mark, s(at(account, "id")), s(at(account, "name")), if truthy(at(account, "personal")) { "personal" } else { "team" }]
        }).collect();
        let any = !rows.is_empty();
        self.output.table(&["", "ID", "NAME", "TYPE"], rows, "No accounts.");
        if any {
            let hint = self.output.muted("Switch with `pingerer use <id or name>`.");
            self.output.puts(hint);
        }
        Ok(EXIT_OK)
    }

    fn command_use(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "use <account | default>", vec![])?;
        let Some(name) = positional.first().cloned() else { return usage("Which account? `pingerer use <id or name>`") };
        no_extra(&positional[1..])?;
        if name == "default" {
            self.config().update(vec![("account", None)])?;
            self.output.puts("Using your default account.");
            return Ok(EXIT_OK);
        }
        let client = self.client()?;
        let account = find_account(&client, &name)?;
        self.config().update(vec![("account", Some(s(at(&account, "id"))))])?;
        if self.output.json {
            self.output.json_value(&account);
            return Ok(EXIT_OK);
        }
        let shown = self.output.bold(s(at(&account, "name")));
        self.output.puts(format!("Using {shown} ({}).", s(at(&account, "id"))));
        Ok(EXIT_OK)
    }

    fn command_status(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, _) = self.parse(&args, "status", vec![])?;
        no_extra(&positional)?;
        let data = at(&self.client()?.get("/api/v1/status", &[])?, "data").clone();
        let down = s(at(&data, "status")) == "down";
        let code = if down { EXIT_DOWN } else { EXIT_OK };
        if self.output.json {
            self.output.json_value(&data);
            return Ok(code);
        }
        let line = format!("{}: {}", self.output.bold(s(dig(&data, &["account", "name"]))), self.output.status(s(at(&data, "status"))));
        self.output.puts(line);
        let fields = vec![("Sites".to_string(), self.count_line(at(&data, "sites"))), ("Monitors".to_string(), self.count_line(at(&data, "monitors")))];
        self.output.fields(fields);
        let down_sites = list(at(&data, "down_sites"));
        if !down_sites.is_empty() {
            self.output.puts("");
            let title = self.output.bold("Down");
            self.output.puts(title);
            let rows = down_sites.iter().map(|site| row![s(at(site, "name")), s(at(site, "host")), ago(at(site, "last_status_change_at"))]).collect();
            self.output.table(&["SITE", "HOST", "SINCE"], rows, "Nothing to show.");
        }
        let incidents = list(at(&data, "open_incidents"));
        self.output.puts("");
        if incidents.is_empty() {
            let none = self.output.muted("No open incidents.");
            self.output.puts(none);
        } else {
            let title = self.output.bold("Open incidents");
            self.output.puts(title);
            let rows = incidents.iter().map(|incident| self.incident_row(incident)).collect();
            self.output.table(&["INCIDENT", "STATE", "KIND", "SITE", "CAUSE", "OPENED"], rows, "Nothing to show.");
        }
        Ok(code)
    }

    // sites [list|add|show|pause|resume|rm]
    fn command_sites(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("sites", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, hits) = self.parse(&args, "sites list [--status STATUS]", vec![value("--status", "STATUS", "Only sites with this status")])?;
                no_extra(&positional)?;
                let sites = self.client()?.get_all("/api/v1/sites", &[("status", last(&hits, "--status"))])?;
                if self.json_out(&Value::Array(sites.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = sites.iter().map(|site| row![s(at(site, "name")), s(at(site, "host")), self.output.status(s(at(site, "status"))), self.output.muted(s(at(site, "id")))]).collect();
                self.output.table(&["NAME", "HOST", "STATUS", "ID"], rows, "No sites yet. Add one with `pingerer sites add yourdomain.com`.");
            }
            "add" => {
                let (positional, hits) = self.parse(&args, "sites add <url> [--checks http,ssl,domain,dns,email_auth] [--every 1m] [--name NAME]", vec![
                    value("--checks", "LIST", "What to check: http, tcp, ping, ssl, domain, dns, email_auth"),
                    value("--every", "DURATION", "How often to check uptime, like 30s, 1m, 5m"),
                    value("--name", "NAME", "A name for the site"),
                    value("--alert-email", "EMAIL", "Send its alerts to this address"),
                    flag("--detect", "Look the site up first to fill in each check's settings"),
                ])?;
                let Some(url) = positional.first().cloned() else { return usage("Which address? `pingerer sites add yourdomain.com`") };
                no_extra(&positional[1..])?;
                let mut body = Map::new();
                for hit in &hits {
                    let given = hit.value.clone().unwrap_or_default();
                    match hit.name {
                        "--checks" => { body.insert("kinds".into(), json!(given.split(',').map(str::trim).collect::<Vec<_>>())); }
                        "--every" => { body.insert("every".into(), json!(given)); }
                        "--name" => { body.insert("name".into(), json!(given)); }
                        "--alert-email" => { body.insert("alert_email".into(), json!(given)); }
                        "--detect" => { body.insert("detect".into(), json!(true)); }
                        _ => {}
                    }
                }
                body.insert("url".into(), json!(url));
                let site = at(&self.client()?.post("/api/v1/sites", Value::Object(body))?, "data").clone();
                if self.json_out(&site) {
                    return Ok(EXIT_OK);
                }
                let name = self.output.bold(s(at(&site, "name")));
                self.output.puts(format!("Added {name} ({}).", s(at(&site, "id"))));
                self.monitors_table(list(at(&site, "monitors")), false);
            }
            "show" => {
                let id = self.one_arg(&args, "sites show <site>")?;
                let site = at(&self.client()?.get(&format!("/api/v1/sites/{}", escape(&id)), &[])?, "data").clone();
                let uptime = at(&self.client()?.get(&format!("/api/v1/sites/{}/uptime", escape(&s(at(&site, "id")))), &[])?, "data").clone();
                if self.output.json {
                    let mut merged = site.clone();
                    if let Some(map) = merged.as_object_mut() {
                        map.insert("uptime".into(), at(&uptime, "windows").clone());
                    }
                    self.output.json_value(&merged);
                    return Ok(EXIT_OK);
                }
                let fields = vec![
                    ("Site".to_string(), self.output.bold(s(at(&site, "name")))),
                    ("URL".to_string(), s(at(&site, "url"))),
                    ("Status".to_string(), self.output.status(s(at(&site, "status")))),
                    ("Uptime".to_string(), uptime_line(at(&uptime, "windows"))),
                    ("ID".to_string(), s(at(&site, "id"))),
                ];
                self.output.fields(fields);
                self.output.puts("");
                self.monitors_table(list(at(&site, "monitors")), false);
            }
            "pause" | "resume" => {
                let id = self.one_arg(&args, &format!("sites {sub} <site>"))?;
                let site = at(&self.client()?.post(&format!("/api/v1/sites/{}/{sub}", escape(&id)), json!({}))?, "data").clone();
                if self.json_out(&site) {
                    return Ok(EXIT_OK);
                }
                let done = if sub == "pause" { "Paused" } else { "Resumed" };
                let line = format!("{done} {}: {}", self.output.bold(s(at(&site, "name"))), self.output.status(s(at(&site, "status"))));
                self.output.puts(line);
            }
            "rm" => {
                let (positional, hits) = self.parse(&args, "sites rm <site> [--yes]", vec![flag("--yes", "Don't ask").short('y')])?;
                let Some(id) = positional.first().cloned() else { return usage("Which site?") };
                no_extra(&positional[1..])?;
                let site = at(&self.client()?.get(&format!("/api/v1/sites/{}", escape(&id)), &[])?, "data").clone();
                self.confirm(&format!("Delete {} ({}), its checks and history?", s(at(&site, "name")), s(at(&site, "host"))), has(&hits, "--yes"))?;
                self.client()?.delete(&format!("/api/v1/sites/{}", escape(&s(at(&site, "id")))))?;
                if !self.output.json {
                    self.output.puts(format!("Deleted {}.", s(at(&site, "name"))));
                }
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    // monitors [list|show|set|pause|resume|test|rm]
    fn command_monitors(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("monitors", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, hits) = self.parse(&args, "monitors list [--site SITE] [--kind KIND] [--status STATUS]", vec![
                    value("--site", "SITE", "Only this site's (id or host)"),
                    value("--kind", "KIND", "Only this kind (http, ssl, heartbeat...)"),
                    value("--status", "STATUS", "Only with this status"),
                ])?;
                no_extra(&positional)?;
                let filters = [("site_id", last(&hits, "--site")), ("kind", last(&hits, "--kind")), ("status", last(&hits, "--status"))];
                let monitors = self.client()?.get_all("/api/v1/monitors", &filters)?;
                if self.json_out(&Value::Array(monitors.clone())) {
                    return Ok(EXIT_OK);
                }
                self.monitors_table(monitors, true);
            }
            "show" => {
                let id = self.one_arg(&args, "monitors show <id>")?;
                let monitor = at(&self.client()?.get(&format!("/api/v1/monitors/{}", escape(&id)), &[])?, "data").clone();
                let uptime = at(&self.client()?.get(&format!("/api/v1/monitors/{}/uptime", escape(&s(at(&monitor, "id")))), &[])?, "data").clone();
                if self.output.json {
                    let mut merged = monitor.clone();
                    if let Some(map) = merged.as_object_mut() {
                        map.insert("uptime".into(), at(&uptime, "windows").clone());
                    }
                    self.output.json_value(&merged);
                    return Ok(EXIT_OK);
                }
                let fields = vec![
                    ("Monitor".to_string(), self.output.bold(s(at(&monitor, "name")))),
                    ("Kind".to_string(), s(at(&monitor, "kind"))),
                    ("Target".to_string(), s(at(&monitor, "target"))),
                    ("Status".to_string(), self.output.status(s(at(&monitor, "status")))),
                    ("Every".to_string(), duration(at(&monitor, "interval_seconds"))),
                    ("Regions".to_string(), list(at(&monitor, "regions")).iter().map(s).collect::<Vec<_>>().join(", ")),
                    ("Last check".to_string(), ago(at(&monitor, "last_checked_at"))),
                    ("Uptime".to_string(), uptime_line(at(&uptime, "windows"))),
                    ("ID".to_string(), s(at(&monitor, "id"))),
                ];
                self.output.fields(fields);
                let settings: Vec<(String, String)> = at(&monitor, "settings").as_object().map(|settings| {
                    settings.iter().filter(|(_, value)| !(value.is_null() || **value == json!({}) || **value == json!([]))).map(|(key, value)| {
                        let shown = match value {
                            Value::Array(items) => items.iter().map(s).collect::<Vec<_>>().join(", "),
                            other => ruby_to_s(other),
                        };
                        (key.clone(), shown)
                    }).collect()
                }).unwrap_or_default();
                if !settings.is_empty() {
                    self.output.puts("");
                    let title = self.output.bold("Settings");
                    self.output.puts(title);
                    self.output.fields(settings);
                }
            }
            "set" => {
                let (positional, hits) = self.parse(&args, "monitors set <id> [--name NAME] [--every 5m] [--renews-on 2027-03-02|none] [--warn-days 30,7]", vec![
                    value("--name", "NAME", "A new name"),
                    value("--every", "DURATION", "How often it checks, like 1m or 5m"),
                    value("--renews-on", "DATE", "A domain check's renewal date, for registries that keep it private (none to clear)"),
                    value("--warn-days", "LIST", "SSL and domain checks: remind this many days before, like 30,7"),
                ])?;
                let mut body = Map::new();
                let mut settings = Map::new();
                for hit in &hits {
                    let given = hit.value.clone().unwrap_or_default();
                    match hit.name {
                        "--name" => { body.insert("name".into(), json!(given)); }
                        "--every" => { body.insert("every".into(), json!(given)); }
                        "--renews-on" => {
                            if given != "none" && !is_date(&given) {
                                return usage("--renews-on takes a date like 2027-03-02, or none");
                            }
                            settings.insert("renews_on".into(), if given == "none" { Value::Null } else { json!(given) });
                        }
                        "--warn-days" => {
                            let days: Option<Vec<i64>> = given.split(',').map(|it| it.trim().parse().ok()).collect();
                            let Some(days) = days else { return usage("--warn-days takes numbers, like 30,7") };
                            settings.insert("warn_days".into(), json!(days));
                        }
                        _ => {}
                    }
                }
                let Some(id) = positional.first().cloned() else { return usage("Which monitor? `pingerer monitors set <id> --renews-on 2027-03-02`") };
                no_extra(&positional[1..])?;
                if !settings.is_empty() {
                    body.insert("settings".into(), Value::Object(settings));
                }
                if body.is_empty() {
                    return usage("Nothing to change. See `pingerer monitors set --help`.");
                }
                let monitor = at(&self.client()?.patch(&format!("/api/v1/monitors/{}", escape(&id)), Value::Object(body))?, "data").clone();
                if self.json_out(&monitor) {
                    return Ok(EXIT_OK);
                }
                let renews = present(dig(&monitor, &["settings", "renews_on"])).map(|it| format!(" Renews on {}.", s(it))).unwrap_or_default();
                let name = self.output.bold(s(at(&monitor, "name")));
                self.output.puts(format!("Updated {name}.{renews}"));
            }
            "pause" | "resume" => {
                let id = self.one_arg(&args, &format!("monitors {sub} <id>"))?;
                let monitor = at(&self.client()?.post(&format!("/api/v1/monitors/{}/{sub}", escape(&id)), json!({}))?, "data").clone();
                if self.json_out(&monitor) {
                    return Ok(EXIT_OK);
                }
                let done = if sub == "pause" { "Paused" } else { "Resumed" };
                let line = format!("{done} {}: {}", self.output.bold(s(at(&monitor, "name"))), self.output.status(s(at(&monitor, "status"))));
                self.output.puts(line);
            }
            "test" => {
                let (positional, hits) = self.parse(&args, "monitors test <id> [--region REGION]", vec![value("--region", "REGION", "Check from this region")])?;
                let Some(id) = positional.first().cloned() else { return usage("Which monitor?") };
                no_extra(&positional[1..])?;
                let region = last(&hits, "--region").map(Value::String).unwrap_or(Value::Null);
                let result = at(&self.client()?.post(&format!("/api/v1/monitors/{}/test", escape(&id)), object(vec![("region", region)]))?, "data").clone();
                let ok = truthy(at(&result, "ok"));
                if self.output.json {
                    self.output.json_value(&result);
                } else if ok {
                    let parts: Vec<String> = [present(at(&result, "status_code")).map(s), present(at(&result, "latency_ms")).map(|ms| format!("in {} ms", s(ms)))].into_iter().flatten().collect();
                    let line = format!("{} {} from {}", self.output.status("ok"), parts.join(" "), s(at(&result, "region")));
                    self.output.puts(line);
                } else {
                    let why = present(at(&result, "error")).or(present(at(&result, "status_code"))).map(s).unwrap_or_default();
                    let line = format!("{} {why} from {}", self.output.status("failed"), s(at(&result, "region")));
                    self.output.puts(line);
                }
                return Ok(if ok { EXIT_OK } else { EXIT_DOWN });
            }
            "rm" => {
                let (positional, hits) = self.parse(&args, "monitors rm <id> [--yes]", vec![flag("--yes", "Don't ask").short('y')])?;
                let Some(id) = positional.first().cloned() else { return usage("Which monitor?") };
                no_extra(&positional[1..])?;
                let monitor = at(&self.client()?.get(&format!("/api/v1/monitors/{}", escape(&id)), &[])?, "data").clone();
                self.confirm(&format!("Delete {} ({}) and its history?", s(at(&monitor, "name")), s(at(&monitor, "kind"))), has(&hits, "--yes"))?;
                self.client()?.delete(&format!("/api/v1/monitors/{}", escape(&s(at(&monitor, "id")))))?;
                if !self.output.json {
                    self.output.puts(format!("Deleted {}.", s(at(&monitor, "name"))));
                }
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    // incidents [list|show|ack|note|resolve]
    fn command_incidents(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("incidents", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, hits) = self.parse(&args, "incidents list [--open] [--state STATE] [--site SITE] [--since 7d] [--limit N]", vec![
                    flag("--open", "Only unresolved ones"),
                    value("--state", "STATE", "open, acknowledged, resolved or unresolved"),
                    value("--site", "SITE", "Only this site's"),
                    value("--since", "DURATION", "Opened in the last 24h, 7d..."),
                    value("--limit", "N", "How many (default 20)"),
                ])?;
                no_extra(&positional)?;
                let mut state = None;
                let mut limit = 20;
                for hit in &hits {
                    match hit.name {
                        "--open" => state = Some("unresolved".to_string()),
                        "--state" => state = hit.value.clone(),
                        "--limit" => limit = options::integer(hit)?,
                        _ => {}
                    }
                }
                let params = [("state", state), ("site_id", last(&hits, "--site")), ("since", last(&hits, "--since")), ("per_page", Some(limit.clamp(1, 100).to_string()))];
                let incidents = list(at(&self.client()?.get("/api/v1/incidents", &params)?, "data"));
                if self.json_out(&Value::Array(incidents.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = incidents.iter().map(|incident| self.incident_row(incident)).collect();
                self.output.table(&["INCIDENT", "STATE", "KIND", "SITE", "CAUSE", "OPENED"], rows, "No incidents.");
            }
            "show" => {
                let id = self.one_arg(&args, "incidents show <id>")?;
                let incident = at(&self.client()?.get(&format!("/api/v1/incidents/{}", escape(&id)), &[])?, "data").clone();
                if self.json_out(&incident) {
                    return Ok(EXIT_OK);
                }
                self.print_incident(&incident);
            }
            "ack" => {
                let id = self.one_arg(&args, "incidents ack <id>")?;
                let incident = at(&self.client()?.post(&format!("/api/v1/incidents/{}/acknowledge", escape(&id)), json!({}))?, "data").clone();
                if self.json_out(&incident) {
                    return Ok(EXIT_OK);
                }
                let line = format!("{} is {}.", s(at(&incident, "reference")), self.output.status(s(at(&incident, "state"))));
                self.output.puts(line);
            }
            "note" => {
                let (mut positional, _) = self.parse(&args, "incidents note <id> <message>", vec![])?;
                if positional.is_empty() {
                    return usage("Which incident?");
                }
                let id = positional.remove(0);
                let message = positional.join(" ");
                if message.trim().is_empty() {
                    return usage(format!("What's the note? `pingerer incidents note {id} Restarted the server`"));
                }
                let incident = at(&self.client()?.post(&format!("/api/v1/incidents/{}/notes", escape(&id)), json!({"message": message}))?, "data").clone();
                if self.json_out(&incident) {
                    return Ok(EXIT_OK);
                }
                self.output.puts(format!("Added a note to {}.", s(at(&incident, "reference"))));
            }
            "resolve" => {
                let (positional, hits) = self.parse(&args, "incidents resolve <id> [--message TEXT]", vec![value("--message", "TEXT", "What fixed it").short('m')])?;
                let Some(id) = positional.first().cloned() else { return usage("Which incident?") };
                no_extra(&positional[1..])?;
                let message = last(&hits, "--message").map(Value::String).unwrap_or(Value::Null);
                let incident = at(&self.client()?.post(&format!("/api/v1/incidents/{}/resolve", escape(&id)), object(vec![("message", message)]))?, "data").clone();
                if self.json_out(&incident) {
                    return Ok(EXIT_OK);
                }
                let line = format!("{} is {}.", s(at(&incident, "reference")), self.output.status(s(at(&incident, "state"))));
                self.output.puts(line);
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    // heartbeats [list|add|ping|rm]
    fn command_heartbeats(&mut self, mut args: Vec<String>) -> Result<i32> {
        let sub = subcommand("heartbeats", &mut args)?;
        match sub.as_str() {
            "list" => {
                let (positional, _) = self.parse(&args, "heartbeats list", vec![])?;
                no_extra(&positional)?;
                let heartbeats = self.client()?.get_all("/api/v1/heartbeats", &[])?;
                if self.json_out(&Value::Array(heartbeats.clone())) {
                    return Ok(EXIT_OK);
                }
                let rows = heartbeats.iter().map(|heartbeat| row![
                    s(at(heartbeat, "name")), self.output.status(s(at(heartbeat, "status"))), duration(at(heartbeat, "expected_every_seconds")),
                    duration(at(heartbeat, "grace_seconds")), ago(at(heartbeat, "last_ping_at")), self.output.muted(s(at(heartbeat, "id")))
                ]).collect();
                self.output.table(&["NAME", "STATUS", "EVERY", "GRACE", "LAST PING", "ID"], rows, "No heartbeats yet. Add one with `pingerer heartbeats add \"Nightly backup\" --every 1d`.");
            }
            "add" => {
                let (positional, hits) = self.parse(&args, "heartbeats add <name> --every 1d [--grace 30m] [--email]", vec![
                    value("--every", "DURATION", "How often the job runs, like 5m, 1h, 1d"),
                    value("--grace", "DURATION", "How late it may be before it's missed (default 30m)"),
                    flag("--email", "Accept pings by email too, where your account has email heartbeats"),
                ])?;
                let name = positional.join(" ");
                if name.trim().is_empty() {
                    return usage("Give it a name: `pingerer heartbeats add \"Nightly backup\" --every 1d`");
                }
                let mut body = Map::new();
                for hit in &hits {
                    match hit.name {
                        "--every" => { body.insert("every".into(), json!(hit.value)); }
                        "--grace" => { body.insert("grace".into(), json!(hit.value)); }
                        "--email" => { body.insert("email_pings".into(), json!(true)); }
                        _ => {}
                    }
                }
                body.insert("name".into(), json!(name));
                let heartbeat = at(&self.client()?.post("/api/v1/heartbeats", Value::Object(body))?, "data").clone();
                if self.json_out(&heartbeat) {
                    return Ok(EXIT_OK);
                }
                let name = self.output.bold(s(at(&heartbeat, "name")));
                self.output.puts(format!("Added {name} ({}), expected every {}.", s(at(&heartbeat, "id")), duration(at(&heartbeat, "expected_every_seconds"))));
                self.output.puts("Ping it when the job finishes:");
                self.output.puts(format!("  curl -fsS {}", s(at(&heartbeat, "ping_url"))));
                if let Some(email) = present(at(&heartbeat, "ping_email")) {
                    self.output.puts(format!("Or email {}", s(email)));
                }
            }
            "ping" => {
                let (positional, hits) = self.parse(&args, "heartbeats ping <token-or-name> [--fail]", vec![flag("--fail", "Report that the job failed")])?;
                let target = positional.join(" ");
                if target.trim().is_empty() {
                    return usage("Which heartbeat? Give its token, id or name.");
                }
                let fail = has(&hits, "--fail");
                let mut url = self.ping_url_for(&target)?;
                if fail {
                    url = format!("{url}/fail");
                }
                // A failure is a POST: it changes something, and link previews only GET.
                let (status, _) = self.transport.call(if fail { "POST" } else { "GET" }, &url, &[("User-Agent".into(), user_agent())], None)?;
                if !(200..300).contains(&status) {
                    return failed(format!("The ping wasn't accepted ({status})."));
                }
                if self.output.json {
                    self.output.json_value(&json!({"pinged": url, "status": status}));
                } else {
                    self.output.puts(format!("Pinged {target}{}.", if fail { " (failed)" } else { "" }));
                }
            }
            "rm" => {
                let (positional, hits) = self.parse(&args, "heartbeats rm <id-or-name> [--yes]", vec![flag("--yes", "Don't ask").short('y')])?;
                let target = positional.join(" ");
                if target.trim().is_empty() {
                    return usage("Which heartbeat?");
                }
                let Some(heartbeat) = self.find_heartbeat(&target)? else { return failed(format!("No heartbeat {target}.")) };
                self.confirm(&format!("Delete the heartbeat {}?", s(at(&heartbeat, "name"))), has(&hits, "--yes"))?;
                self.client()?.delete(&format!("/api/v1/heartbeats/{}", escape(&s(at(&heartbeat, "id")))))?;
                if !self.output.json {
                    self.output.puts(format!("Deleted {}.", s(at(&heartbeat, "name"))));
                }
            }
            _ => {}
        }
        Ok(EXIT_OK)
    }

    fn command_results(&mut self, args: Vec<String>) -> Result<i32> {
        let (positional, hits) = self.parse(&args, "results <monitor> [--since 24h] [--until TIME] [--region REGION] [--failed] [--limit N]", vec![
            value("--since", "TIME", "From this long ago (24h, 7d) or an ISO 8601 time"),
            value("--until", "TIME", "Up to this ISO 8601 time"),
            value("--region", "REGION", "Only from this region (yyz, iad, fsn, sgp)"),
            flag("--failed", "Only failed checks"),
            value("--limit", "N", "How many (default 50, up to 500)"),
        ])?;
        let Some(id) = positional.first().cloned() else { return usage("Which monitor? `pingerer results <monitor id>`") };
        no_extra(&positional[1..])?;
        let mut limit = 50;
        for hit in &hits {
            if hit.name == "--limit" {
                limit = options::integer(hit)?;
            }
        }
        let params = [
            ("since", Some(last(&hits, "--since").unwrap_or_else(|| "24h".into()))),
            ("until", last(&hits, "--until")),
            ("region", last(&hits, "--region")),
            ("ok", has(&hits, "--failed").then(|| "false".to_string())),
            ("per_page", Some(limit.clamp(1, 500).to_string())),
        ];
        let results = list(at(&self.client()?.get(&format!("/api/v1/monitors/{}/results", escape(&id)), &params)?, "data"));
        if self.json_out(&Value::Array(results.clone())) {
            return Ok(EXIT_OK);
        }
        let rows = results.iter().map(|result| row![
            local_time(at(result, "checked_at")), s(at(result, "region")), self.output.status(if truthy(at(result, "ok")) { "ok" } else { "failed" }),
            s(at(result, "status_code")), present(at(result, "latency_ms")).map(|ms| format!("{} ms", s(ms))).unwrap_or_default(), s(at(result, "error"))
        ]).collect();
        self.output.table(&["CHECKED", "REGION", "RESULT", "CODE", "TIME", "ERROR"], rows, "No results in that time.");
        Ok(EXIT_OK)
    }

    // --- Helpers -----------------------------------------------------------------

    /// Parses the command's options (and the global ones, anywhere) and
    /// returns the arguments left and the command's own options.
    fn parse(&mut self, args: &[String], usage_text: &str, opts: Vec<Opt>) -> Result<(Vec<String>, Vec<Hit>)> {
        let mut all = opts;
        all.extend(global_opts());
        all.push(flag("--help", "Show this help").short('h'));
        let (positional, hits) = options::parse(args, &all)?;
        self.apply_global(&hits);
        if self.help {
            self.output.puts(format!("Usage: pingerer {usage_text}"));
            self.output.puts(options::summary(&all).trim_end());
            return Err(Error::HelpShown);
        }
        let own = hits.into_iter().filter(|hit| !["--json", "--account", "--url", "--color", "--help"].contains(&hit.name)).collect();
        Ok((positional, own))
    }

    fn print_command_help(&mut self, command: &str) -> Result<i32> {
        let text = COMMANDS.iter().find(|(name, _)| *name == command).map(|(_, text)| *text).unwrap_or("");
        self.output.puts(format!("pingerer {command}: {text}"));
        match SUBCOMMANDS.iter().find(|(name, _)| *name == command) {
            Some((_, subs)) => {
                self.output.puts(format!("Subcommands: {} (default {}).", subs.join(", "), subs[0]));
                self.output.puts(format!("Run `pingerer {command} <subcommand> --help` for its options."));
            }
            None => self.output.puts(format!("Run `pingerer {command} --help` for its options.")),
        }
        Ok(EXIT_OK)
    }

    fn one_arg(&mut self, args: &[String], usage_text: &str) -> Result<String> {
        let (positional, _) = self.parse(args, usage_text, vec![])?;
        if positional.len() != 1 {
            return usage(format!("Usage: pingerer {usage_text}"));
        }
        Ok(positional[0].clone())
    }

    /// Prints data as JSON when --json was given; true when it did.
    fn json_out(&mut self, data: &Value) -> bool {
        if self.output.json {
            self.output.json_value(data);
        }
        self.output.json
    }

    fn config(&mut self) -> &mut Config {
        if self.config.is_none() {
            self.config = Some(Config::new(Config::default_path(&self.env), self.env.clone()));
        }
        self.config.as_mut().expect("a config")
    }

    fn base_url(&mut self) -> String {
        let url = match &self.url_option {
            Some(url) => url.clone(),
            None => self.config().url(),
        };
        url.trim_end_matches('/').to_string()
    }

    fn client(&mut self) -> Result<Rc<Client>> {
        if let Some(client) = &self.client {
            return Ok(client.clone());
        }
        let Some(token) = self.config().token() else { return failed("Not logged in. Run `pingerer login`, or set PINGERER_TOKEN.") };
        let base_url = self.base_url();
        let mut client = Client::new(&base_url, Some(token), self.transport.clone());
        if let Some(account) = self.account_option.clone() {
            client.account = Some(resolve_account(&client, &account)?);
        }
        if client.account.is_none() {
            client.account = self.config().account();
        }
        let client = Rc::new(client);
        self.client = Some(client.clone());
        Ok(client)
    }

    fn find_heartbeat(&mut self, target: &str) -> Result<Option<Value>> {
        let heartbeats = self.client()?.get_all("/api/v1/heartbeats", &[])?;
        Ok(heartbeats.into_iter().find(|heartbeat| s(at(heartbeat, "id")) == target || s(at(heartbeat, "name")).to_lowercase() == target.to_lowercase()))
    }

    /// A URL is used as is; an id or name is looked up (when logged in);
    /// anything else is the heartbeat's token.
    fn ping_url_for(&mut self, target: &str) -> Result<String> {
        if target.starts_with("http://") || target.starts_with("https://") {
            return Ok(target.to_string());
        }
        if self.config().token().is_some() {
            if let Some(heartbeat) = self.find_heartbeat(target)? {
                return Ok(s(at(&heartbeat, "ping_url")));
            }
            if target.starts_with("mon_") {
                return failed(format!("No heartbeat {target}."));
            }
        }
        if target.is_empty() || !target.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return usage(format!("{target} isn't a heartbeat token"));
        }
        Ok(format!("{}/hb/{target}", self.base_url()))
    }

    fn confirm(&mut self, question: &str, yes: bool) -> Result<()> {
        if yes {
            return Ok(());
        }
        if !self.input_tty {
            return usage(format!("{question} Add --yes to confirm."));
        }
        self.output.print(format!("{question} [y/N] "));
        let mut answer = String::new();
        let _ = self.input.read_line(&mut answer);
        if !["y", "yes"].contains(&answer.trim().to_lowercase().as_str()) {
            return failed("Cancelled.");
        }
        Ok(())
    }

    fn prompt_token(&mut self) -> Result<String> {
        let url = self.base_url();
        self.output.print(format!("API token (from {url}/api_tokens): "));
        let token = if self.input_tty {
            
            rpassword::read_password().unwrap_or_default()
        } else {
            let mut line = String::new();
            let _ = self.input.read_line(&mut line);
            line
        };
        Ok(token.trim().to_string())
    }

    fn monitors_table(&mut self, monitors: Vec<Value>, target: bool) {
        let headers: &[&str] = if target { &["NAME", "KIND", "TARGET", "STATUS", "EVERY", "ID"] } else { &["NAME", "KIND", "STATUS", "EVERY", "ID"] };
        let rows = monitors.iter().map(|monitor| {
            let mut cells = row![s(at(monitor, "name")), s(at(monitor, "kind"))];
            if target {
                cells.push(s(at(monitor, "target")));
            }
            cells.extend(row![self.output.status(s(at(monitor, "status"))), duration(at(monitor, "interval_seconds")), self.output.muted(s(at(monitor, "id")))]);
            cells
        }).collect();
        self.output.table(headers, rows, "No monitors.");
    }

    fn incident_row(&self, incident: &Value) -> Vec<String> {
        let site = present(at(incident, "site_name")).or(present(at(incident, "monitor_name"))).map(s).unwrap_or_default();
        row![s(at(incident, "reference")), self.output.status(s(at(incident, "state"))), s(at(incident, "kind")), site,
            s(at(incident, "cause")).chars().take(50).collect::<String>(), ago(at(incident, "opened_at"))]
    }

    fn print_incident(&mut self, incident: &Value) {
        let fields = vec![
            ("Incident".to_string(), self.output.bold(format!("{} {}", s(at(incident, "reference")), s(at(incident, "kind"))))),
            ("State".to_string(), self.output.status(s(at(incident, "state")))),
            ("Site".to_string(), s(at(incident, "site_name"))),
            ("Monitor".to_string(), s(at(incident, "monitor_name"))),
            ("Cause".to_string(), s(at(incident, "cause"))),
            ("Opened".to_string(), local_time(at(incident, "opened_at"))),
            ("Duration".to_string(), duration(at(incident, "duration_seconds"))),
            ("ID".to_string(), s(at(incident, "id"))),
        ];
        self.output.fields(fields);
        if let Some(trouble) = present(at(incident, "provider_trouble")) {
            let name = s(at(trouble, "name"));
            self.output.puts("");
            let line = format!("{} lots of other sites on {name} went down at the same time, so it's most likely {name}.", self.output.bold("Probably not you:"));
            self.output.puts(line);
        }
        let events = list(at(incident, "events"));
        if events.is_empty() {
            return;
        }
        self.output.puts("");
        let title = self.output.bold("Timeline");
        self.output.puts(title);
        let rows = events.iter().map(|event| {
            let who = present(dig(event, &["user", "name"])).or(present(at(event, "region"))).map(s).unwrap_or_default();
            row![local_time(at(event, "created_at")), s(at(event, "kind")), who, s(at(event, "message"))]
        }).collect();
        self.output.table(&["WHEN", "WHAT", "WHO", "DETAIL"], rows, "Nothing to show.");
    }

    fn count_line(&self, counts: &Value) -> String {
        let parts: Vec<String> = ["down", "degraded", "up", "paused", "pending"].iter().filter_map(|status| {
            let count = int(at(counts, status));
            (count > 0 || ["down", "up"].contains(status)).then(|| format!("{count} {}", self.output.status(status)))
        }).collect();
        format!("{} ({})", parts.join(" · "), int(at(counts, "total")))
    }

    fn sleep(&mut self, seconds: f64) {
        (self.sleeper)(seconds);
    }
}

fn known(command: &str) -> bool {
    COMMANDS.iter().any(|(name, _)| *name == command)
}

fn subcommand(command: &str, args: &mut Vec<String>) -> Result<String> {
    if args.is_empty() || args[0].starts_with('-') {
        let default = SUBCOMMANDS.iter().find(|(name, _)| *name == command).map(|(_, subs)| subs[0]).unwrap_or("list");
        // quiet-hours shows by default; the others list.
        return Ok(if command == "quiet-hours" { "list".into() } else { default.into() });
    }
    let sub = args.remove(0);
    let subs = SUBCOMMANDS.iter().find(|(name, _)| *name == command).map(|(_, subs)| *subs).unwrap_or(&[]);
    if !subs.contains(&sub.as_str()) {
        return usage(format!("Unknown {command} command: {sub} (try {})", subs.join(", ")));
    }
    Ok(sub)
}

fn no_extra(positional: &[String]) -> Result<()> {
    match positional.first() {
        Some(extra) => usage(format!("Unexpected argument: {extra}")),
        None => Ok(()),
    }
}

/// The last value given for an option.
fn last(hits: &[Hit], name: &str) -> Option<String> {
    hits.iter().rev().find(|hit| hit.name == name).and_then(|hit| hit.value.clone())
}

fn has(hits: &[Hit], name: &str) -> bool {
    hits.iter().any(|hit| hit.name == name)
}

fn accounts_list(response: Value) -> Vec<Value> {
    match response {
        Value::Object(_) => list(at(&response, "data")),
        other => list(&other),
    }
}

/// An id passes through; a name is looked up in your accounts.
fn resolve_account(client: &Client, given: &str) -> Result<String> {
    let numeric = !given.is_empty() && given.chars().all(|c| c.is_ascii_digit());
    let prefixed = given.strip_prefix("acct_").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    if numeric || prefixed {
        return Ok(given.to_string());
    }
    Ok(s(at(&find_account(client, given)?, "id")))
}

fn find_account(client: &Client, given: &str) -> Result<Value> {
    let accounts = accounts_list(client.get("/api/v1/accounts", &[])?);
    if let Some(found) = accounts.iter().find(|account| [s(at(account, "id")), s(at(account, "prefix_id"))].contains(&given.to_string())) {
        return Ok(found.clone());
    }
    let named: Vec<&Value> = accounts.iter().filter(|account| s(at(account, "name")).to_lowercase() == given.to_lowercase()).collect();
    match named.as_slice() {
        [only] => Ok((*only).clone()),
        [] => failed(format!("You don't have an account {given}. See `pingerer accounts`.")),
        _ => failed(format!("More than one account is called {given}; use its id.")),
    }
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' && bytes.iter().enumerate().all(|(index, b)| index == 4 || index == 7 || b.is_ascii_digit())
}

/// Ruby's to_s for a settings value (a hash prints as Ruby would, near enough).
fn ruby_to_s(value: &Value) -> String {
    match value {
        Value::Object(_) => value.to_string(),
        other => s(other),
    }
}

pub(crate) fn uptime_line(windows: &Value) -> String {
    let Some(windows) = windows.as_object() else { return String::new() };
    windows.iter().map(|(name, window)| {
        match at(window, "uptime_percent").as_f64() {
            Some(percent) => format!("{name} {percent:.2}%"),
            None => format!("{name} —"),
        }
    }).collect::<Vec<_>>().join(" · ")
}

/// 3600 is "1h", 86400 "1d", 90 "90s", 5400 "1.5h".
pub(crate) fn duration(seconds: &Value) -> String {
    if seconds.is_null() {
        return String::new();
    }
    duration_of(int(seconds))
}

pub(crate) fn duration_of(seconds: i64) -> String {
    for (size, unit) in [(604_800, "w"), (86_400, "d"), (3600, "h"), (60, "m")] {
        if seconds >= size && seconds % size == 0 {
            return format!("{}{unit}", seconds / size);
        }
    }
    if seconds >= 3600 {
        format!("{:.1}h", (seconds as f64 / 3600.0 * 10.0).round() / 10.0)
    } else {
        format!("{seconds}s")
    }
}

/// "5m ago", "2h from now", "just now".
pub(crate) fn ago(time: &Value) -> String {
    let text = s(time);
    if text.is_empty() {
        return "—".into();
    }
    match chrono::DateTime::parse_from_rfc3339(&text) {
        Ok(at) => {
            let seconds = ((chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_milliseconds() as f64 / 1000.0).round() as i64;
            if seconds.abs() < 45 {
                return "just now".into();
            }
            format!("{} {}", duration_words(seconds.abs()), if seconds < 0 { "from now" } else { "ago" })
        }
        Err(_) => text,
    }
}

pub(crate) fn duration_words(seconds: i64) -> String {
    let seconds = seconds as f64;
    if seconds < 3600.0 {
        format!("{}m", (seconds / 60.0).round())
    } else if seconds < 86_400.0 {
        format!("{}h", (seconds / 3600.0).round())
    } else {
        format!("{}d", (seconds / 86_400.0).round())
    }
}

/// An ISO 8601 time in this computer's time zone: "2026-10-01 08:00:00".
pub(crate) fn local_time(time: &Value) -> String {
    if time.is_null() {
        return "—".into();
    }
    let text = s(time);
    match chrono::DateTime::parse_from_rfc3339(&text) {
        Ok(at) => at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string(),
        Err(_) => text,
    }
}
