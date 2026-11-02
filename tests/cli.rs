//! The Ruby gem's cli_test.rb, clients_test.rb, config_test.rb and
//! client_test.rb, against the Rust CLI.

mod common;

use common::{now_iso, ok, World};
use serde_json::{json, Value};
use std::collections::HashMap;

fn status_data(status: &str, down_sites: Value, incidents: Value) -> Value {
    let down = down_sites.as_array().map_or(0, Vec::len);
    json!({"data": {"account": {"id": "acct_1", "name": "Company"}, "status": status,
        "sites": {"up": 1, "down": down, "total": 1 + down}, "monitors": {"up": 3, "total": 3},
        "down_sites": down_sites, "open_incidents": incidents}})
}

fn up() -> Value {
    status_data("up", json!([]), json!([]))
}

#[test]
fn help_and_version() {
    let world = World::new();
    let (code, out, _) = world.run(&["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("heartbeats"));
    for command in ["health", "quiet-hours", "link-preview", "weather"] {
        assert!(out.contains(command), "{command}");
    }

    let (code, out, _) = world.run(&["--version"]);
    assert_eq!(code, 0);
    assert_eq!(out, format!("pingerer {}\n", pingerer::VERSION));
}

#[test]
fn unknown_command_and_option_are_usage_errors() {
    let world = World::new();
    let (code, _, err) = world.run(&["frobnicate"]);
    assert_eq!(code, 2);
    assert!(err.contains("Unknown command"));

    world.logged_in();
    let (code, _, err) = world.run(&["sites", "--bogus"]);
    assert_eq!(code, 2);
    assert!(err.contains("invalid option"));
    assert!(err.contains("pingerer help sites"));
}

#[test]
fn command_help_exits_0() {
    let world = World::new();
    let (code, out, _) = world.run(&["sites", "add", "--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Usage: pingerer sites add <url>"));
    assert!(out.contains("--checks LIST"));

    let (code, out, _) = world.run(&["help", "monitors"]);
    assert_eq!(code, 0);
    assert!(out.contains("Subcommands: list, show, set"));
}

#[test]
fn needs_a_login() {
    let world = World::new();
    let (code, _, err) = world.run(&["status"]);
    assert_eq!(code, 1);
    assert!(err.contains("pingerer login"));
}

#[test]
fn login_verifies_and_saves_the_token_privately() {
    let world = World::new();
    world.api.on("GET", "/api/v1/me", |_, _| ok(json!({"id": 1, "name": "User One"})));

    let (code, out, _) = world.run_with(&["login", "--url", "https://pingerer.test"], "tok_abc\n", &[]);
    assert_eq!(code, 0);
    assert!(out.contains("Logged in to https://pingerer.test as User One"), "{out}");
    assert_eq!(world.api.last().header("Authorization").as_deref(), Some("Bearer tok_abc"));
    assert_eq!(world.config().token().as_deref(), Some("tok_abc"));
    assert_eq!(world.config().url(), "https://pingerer.test");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(world.config_path()).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(world.config_path().parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn login_with_a_bad_token_saves_nothing() {
    let world = World::new();
    world.api.on("GET", "/api/v1/me", |_, _| (401, Value::Null));
    let (code, _, err) = world.run(&["login", "--token", "nope"]);
    assert_eq!(code, 1);
    assert!(err.contains("pingerer login"));
    assert_eq!(world.config().token(), None);
}

#[test]
fn logout_forgets_the_token() {
    let world = World::new();
    world.logged_in();
    let (code, out, _) = world.run(&["logout"]);
    assert_eq!(code, 0);
    assert!(out.contains("Logged out"));
    assert_eq!(world.config().token(), None);
}

#[test]
fn env_token_and_url_win() {
    let world = World::new();
    world.config().update(vec![("token", Some("file_token".into())), ("url", Some("https://file.test".into()))]).unwrap();
    world.api.on("GET", "/api/v1/status", |_, _| ok(up()));
    world.run_with(&["status"], "", &[("PINGERER_TOKEN", "env_token"), ("PINGERER_URL", "https://env.test")]);
    assert_eq!(world.api.last().header("Authorization").as_deref(), Some("Bearer env_token"));
    assert_eq!(world.api.last().host(), "env.test");
}

#[test]
fn status_exits_3_when_something_is_down() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/status", |_, _| ok(status_data("down",
        json!([{"name": "Shop", "host": "shop.test", "last_status_change_at": now_iso()}]),
        json!([{"reference": "INC-0042", "state": "open", "kind": "down", "site_name": "Shop", "cause": "502", "opened_at": now_iso()}]))));
    let (code, out, _) = world.run(&["status"]);
    assert_eq!(code, 3);
    assert!(out.contains("Company: down"));
    assert!(out.contains("INC-0042"));
    assert!(out.contains("just now"));
    assert!(!out.contains('\x1b'), "no color when not a terminal");
}

#[test]
fn status_json_and_forced_color() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/status", |_, _| ok(up()));
    let (code, out, _) = world.run(&["status", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["status"], "up");

    let (_, out, _) = world.run(&["status", "--color"]);
    assert!(out.contains("\x1b[32mup\x1b[0m"));
    let (_, out, _) = world.run_with(&["status"], "", &[("NO_COLOR", "1")]);
    assert!(!out.contains('\x1b'));
}

#[test]
fn sites_list_is_an_aligned_table() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/sites", |_, _| ok(json!({"data": [
        {"id": "site_1", "name": "Your Bakery", "host": "yourbakery.com", "status": "up"},
        {"id": "site_2", "name": "Shop", "host": "shop.yourbakery.com", "status": "down"}], "meta": {"next_page": null}})));
    let (code, out, _) = world.run(&["sites"]);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(regex::Regex::new(r"^NAME\s+HOST\s+STATUS\s+ID").unwrap().is_match(lines[0]));
    assert_eq!(lines[1].find("yourbakery.com"), lines[0].find("HOST"));
    assert_eq!(lines[2].find("down"), lines[0].find("STATUS"));
}

#[test]
fn sites_list_follows_pages() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/sites", |request, _| {
        let page: i64 = request.params()["page"].parse().unwrap();
        ok(json!({"data": [{"id": format!("site_{page}"), "name": format!("S{page}"), "host": format!("s{page}.test"), "status": "up"}],
            "meta": {"next_page": if page < 2 { json!(page + 1) } else { Value::Null }}}))
    });
    let (_, out, _) = world.run(&["sites", "list", "--json"]);
    let ids: Vec<String> = serde_json::from_str::<Vec<Value>>(&out).unwrap().iter().map(|it| it["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec!["site_1", "site_2"]);
}

#[test]
fn sites_add_sends_checks_and_interval() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/sites", |request, _| (201, json!({"data": {"id": "site_9", "name": request.json()["url"],
        "monitors": [{"id": "mon_1", "name": "Uptime", "kind": "http", "status": "pending", "interval_seconds": 60}]}})));
    let (code, out, _) = world.run(&["sites", "add", "yourbakery.com", "--checks", "http,ssl", "--every", "1m"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"url": "yourbakery.com", "kinds": ["http", "ssl"], "every": "1m"}));
    assert!(out.contains("Added yourbakery.com (site_9)"));
    assert!(out.contains("1m"));
}

#[test]
fn plan_limit_errors_explain() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/sites", |_, _| (422, json!({"error": "plan_limit", "message": "Your plan allows 5 active monitors",
        "details": {"base": ["Your plan allows 5 active monitors"], "url": ["is taken"]}})));
    let (code, _, err) = world.run(&["sites", "add", "yourbakery.com"]);
    assert_eq!(code, 1);
    assert!(err.contains("Your plan allows 5"));
    assert!(err.contains("  url: is taken"));
    assert!(err.contains("Upgrade your plan"));
}

#[test]
fn sites_show_by_host() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/sites/yourbakery.com", |_, _| ok(json!({"data": {"id": "site_1", "name": "Your Bakery", "url": "https://yourbakery.com/", "status": "up", "monitors": []}})));
    world.api.on("GET", "/api/v1/sites/site_1/uptime", |_, _| ok(json!({"data": {"windows": {"24h": {"uptime_percent": 99.5}, "7d": {"uptime_percent": null}}}})));
    let (code, out, _) = world.run(&["sites", "show", "yourbakery.com"]);
    assert_eq!(code, 0);
    assert!(out.contains("24h 99.50% · 7d —"), "{out}");
}

#[test]
fn rm_needs_yes_without_a_terminal() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/sites/site_1", |_, _| ok(json!({"data": {"id": "site_1", "name": "Bakery", "host": "yourbakery.com"}})));
    world.api.on("DELETE", "/api/v1/sites/site_1", |_, _| (204, Value::Null));
    let (code, _, err) = world.run(&["sites", "rm", "site_1"]);
    assert_eq!(code, 2);
    assert!(err.contains("--yes"));
    let (code, _, _) = world.run(&["sites", "rm", "site_1", "--yes"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().method, "DELETE");
}

#[test]
fn monitors_test_exits_3_when_the_check_fails() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/monitors/mon_1/test", |_, _| ok(json!({"data": {"ok": false, "error": "Connection refused", "region": "yyz"}})));
    let (code, out, _) = world.run(&["monitors", "test", "mon_1"]);
    assert_eq!(code, 3);
    assert!(out.contains("failed Connection refused from yyz"));
    assert_eq!(world.api.last().json(), json!({}));
}

#[test]
fn incidents_list_open() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/incidents", |_, _| ok(json!({"data": [{"reference": "INC-0042", "state": "open", "kind": "down", "site_name": "Shop", "cause": "502", "opened_at": now_iso()}]})));
    let (code, out, _) = world.run(&["incidents", "list", "--open"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().params()["state"], "unresolved");
    assert_eq!(world.api.last().params()["per_page"], "20");
    assert!(out.contains("INC-0042"));
}

#[test]
fn incident_note_and_resolve() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", r"^/api/v1/incidents/INC-0042/(notes|resolve)$", |request, captures| {
        let notes = captures[1] == "notes";
        (if notes { 201 } else { 200 }, json!({"data": {"reference": "INC-0042", "state": if notes { "open" } else { "resolved" }, "body": request.json()}}))
    });
    let (code, _, _) = world.run(&["incidents", "note", "INC-0042", "Restarted", "nginx"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"message": "Restarted nginx"}));
    let (code, out, _) = world.run(&["incidents", "resolve", "INC-0042", "-m", "Fixed"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"message": "Fixed"}));
    assert!(out.contains("resolved"));
}

#[test]
fn heartbeats_add_shows_the_ping_url() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/heartbeats", |_, _| (201, json!({"data": {"id": "mon_7", "name": "Nightly backup", "expected_every_seconds": 86_400, "ping_url": "https://pingerer.test/hb/abc"}})));
    let (code, out, _) = world.run(&["heartbeats", "add", "Nightly", "backup", "--every", "1d", "--grace", "30m"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"name": "Nightly backup", "every": "1d", "grace": "30m"}));
    assert!(out.contains("expected every 1d"));
    assert!(out.contains("curl -fsS https://pingerer.test/hb/abc"));
}

#[test]
fn heartbeats_ping_by_name_and_by_token() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/heartbeats", |_, _| ok(json!({"data": [{"id": "mon_7", "name": "Nightly backup", "ping_url": "https://pingerer.test/hb/abc"}], "meta": {}})));
    world.api.on("GET", r"^/hb/", |_, _| ok(json!({})));
    let (code, _, _) = world.run(&["heartbeats", "ping", "nightly backup"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().path(), "/hb/abc");
    assert_eq!(world.api.last().header("Authorization"), None, "pings don't send the token");

    world.api.on("POST", r"^/hb/", |_, _| ok(json!({})));
    let (code, _, _) = world.run(&["heartbeats", "ping", "zzToken123", "--fail"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().path(), "/hb/zzToken123/fail");
    assert_eq!(world.api.last().method, "POST");
}

#[test]
fn heartbeat_ping_works_without_login() {
    let world = World::new();
    world.api.on("GET", r"^/hb/", |_, _| ok(json!({})));
    let (code, _, _) = world.run_with(&["heartbeats", "ping", "zzToken123"], "", &[("PINGERER_URL", "https://pingerer.test")]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().url, "https://pingerer.test/hb/zzToken123");
}

#[test]
fn results_since() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/monitors/mon_1/results", |_, _| ok(json!({"data": [{"checked_at": "2026-10-01T12:00:00Z", "region": "yyz", "ok": true, "status_code": 200, "latency_ms": 180}]})));
    let (code, out, _) = world.run(&["results", "mon_1", "--since", "7d", "--failed"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().params(), HashMap::from([("since".into(), "7d".into()), ("ok".into(), "false".into()), ("per_page".into(), "50".into())]));
    assert!(out.contains("180 ms"));
}

#[test]
fn use_saves_the_account_and_sends_it() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/accounts", |_, _| ok(json!([{"id": 1, "name": "Personal", "personal": true}, {"id": 7, "name": "Company", "personal": false}])));
    world.api.on("GET", "/api/v1/status", |_, _| ok(up()));
    let (code, out, _) = world.run(&["use", "company"]);
    assert_eq!(code, 0);
    assert!(out.contains("Using Company (7)"));
    assert_eq!(world.config().account().as_deref(), Some("7"));

    world.run(&["status"]);
    assert_eq!(world.api.last().header("Pingerer-Account").as_deref(), Some("7"));
    world.run(&["status", "--account", "Personal"]);
    assert_eq!(world.api.last().header("Pingerer-Account").as_deref(), Some("1"));
    world.run(&["--account", "acct_9", "status"]);
    assert_eq!(world.api.last().header("Pingerer-Account").as_deref(), Some("acct_9"));

    let (_, out, _) = world.run(&["accounts"]);
    assert!(out.lines().any(|line| line.starts_with('*') && line.contains("Company")), "{out}");
}

#[test]
fn unknown_and_suspended_accounts() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/accounts", |_, _| ok(json!([{"id": 1, "name": "Personal"}])));
    let (code, _, err) = world.run(&["use", "nope"]);
    assert_eq!(code, 1);
    assert!(err.contains("You don't have an account nope"));

    world.api.on("GET", "/api/v1/status", |_, _| (403, json!({"error": "account_suspended", "message": "This account is paused by Pingerer."})));
    let (code, _, err) = world.run(&["status"]);
    assert_eq!(code, 1);
    assert!(err.contains("paused by Pingerer"));
}

#[test]
fn clients_list_add_and_usage() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/clients", |_, _| ok(json!({"data": [{"id": "acct_1", "name": "Your Bakery", "state": "down", "sites": 6, "uptime_30d": 99.94, "open_incidents": 2}], "meta": {}})));
    let (code, out, _) = world.run(&["clients", "list"]);
    assert_eq!(code, 0);
    assert!(out.contains("Your Bakery") && out.contains("99.94%") && out.contains("acct_1"));

    world.api.on("GET", "/api/v1/clients", |_, _| ok(json!({"data": [], "meta": {}})));
    let (_, out, _) = world.run(&["clients"]);
    assert!(out.contains("No clients yet"));

    world.api.on("POST", "/api/v1/clients", |request, _| {
        let mut data = request.json();
        data["id"] = json!("acct_2");
        (201, json!({"data": data}))
    });
    let (code, out, _) = world.run(&["clients", "add", "Harbor", "Dental"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"name": "Harbor Dental"}));
    assert!(out.contains("Added") && out.contains("acct_2"));

    let (code, _, err) = world.run(&["clients", "add"]);
    assert_eq!(code, 2);
    assert!(err.contains("clients add <name>"));
}

#[test]
fn config_paths_and_default_url() {
    let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
    assert_eq!(pingerer::config::Config::default_path(&env(&[("XDG_CONFIG_HOME", "/x")])), std::path::PathBuf::from("/x/pingerer/config.json"));
    assert_eq!(pingerer::config::Config::default_path(&env(&[("HOME", "/home/sam")])), std::path::PathBuf::from("/home/sam/.config/pingerer/config.json"));
    let world = World::new();
    assert_eq!(world.config().url(), "https://pingerer.com");
}

/// One real HTTP exchange over a local socket, to check the transport end to end.
#[test]
fn http_transport_against_a_local_server() {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let length: usize = head.lines().find_map(|line| line.to_lowercase().strip_prefix("content-length: ").map(|it| it.trim().parse().unwrap())).unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let payload = r#"{"data":{"ok":true}}"#;
        let mut stream = stream;
        write!(stream, "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len()).unwrap();
        (head, String::from_utf8(body).unwrap())
    });

    let transport: std::rc::Rc<dyn pingerer::client::Transport> = std::rc::Rc::new(pingerer::client::HttpTransport::default());
    let mut client = pingerer::client::Client::new(&format!("http://127.0.0.1:{port}"), Some("tok".into()), transport.clone());
    client.account = Some("acct_1".into());
    let response = client.post("/api/v1/sites", json!({"url": "yourbakery.com"})).unwrap();
    let (head, body) = server.join().unwrap();
    assert_eq!(response, json!({"data": {"ok": true}}));
    assert!(head.starts_with("POST /api/v1/sites HTTP/1.1"), "{head}");
    assert!(head.to_lowercase().contains("authorization: bearer tok"));
    assert!(head.to_lowercase().contains("pingerer-account: acct_1"));
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({"url": "yourbakery.com"}));

    // A server that isn't there.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let client = pingerer::client::Client::new(&format!("http://127.0.0.1:{closed}"), Some("tok".into()), transport);
    assert!(matches!(client.get("/api/v1/me", &[]), Err(pingerer::errors::Error::Failed(message)) if message.starts_with("Couldn't reach http://127.0.0.1")));
}

/// The token never crosses the network in the clear.
#[test]
fn the_token_is_not_sent_over_plain_http_to_another_machine() {
    let transport: std::rc::Rc<dyn pingerer::client::Transport> = std::rc::Rc::new(pingerer::client::HttpTransport::default());
    let client = pingerer::client::Client::new("http://pingerer.example.com", Some("secret-abc123".into()), transport.clone());
    let Err(pingerer::errors::Error::Failed(message)) = client.get("/api/v1/me", &[]) else { panic!("refused") };
    assert!(message.contains("plain http") && !message.contains("secret-abc123"));
    let other = pingerer::client::Client::new("ftp://pingerer.example.com", Some("tok".into()), transport);
    assert!(other.get("/api/v1/me", &[]).is_err());

    use pingerer::client::local;
    assert!(local("localhost") && local("127.0.0.1") && local("pingerer.localhost") && local("pingerer.test"));
    assert!(!local("pingerer.com") && !local("localhost.evil.com"));
}

#[test]
fn error_responses_become_api_errors() {
    let world = World::new();
    world.api.on("GET", "/api/v1/sites/x", |_, _| (404, json!({"error": "not_found", "message": "That site wasn't found."})));
    let client = pingerer::client::Client::new("https://p.test", Some("t".into()), world.api.clone());
    let Err(pingerer::errors::Error::Api(error)) = client.get("/api/v1/sites/x", &[]) else { panic!("an API error") };
    assert_eq!((error.status, error.code.as_str(), error.message.as_str()), (404, "not_found", "That site wasn't found."));
}
