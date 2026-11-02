//! The Ruby gem's deploy_commands_test.rb and maintenance_test.rb: what the
//! GitHub Action does, for other CI systems.

mod common;

use common::{ok, World};
use serde_json::{json, Value};
use std::cell::Cell;
use std::rc::Rc;

fn window() -> Value {
    json!({"id": "mw_7", "title": "Deploy", "state": "in_progress", "all_sites": false,
        "sites": [{"id": "site_1", "host": "yourbakery.com"}], "recurrence": "none", "days": [],
        "starts_at": "2030-10-14T06:00:00Z", "ends_at": "2030-10-14T06:15:00Z"})
}

fn monitors() -> Value {
    json!([
        {"id": "mon_1", "name": "Uptime", "kind": "http", "site_id": "site_1", "paused": false, "status": "up"},
        {"id": "mon_2", "name": "SSL certificate", "kind": "ssl", "site_id": "site_1", "paused": false, "status": "up"},
        {"id": "mon_3", "name": "Port 22", "kind": "tcp", "site_id": "site_1", "paused": true, "status": "paused"}
    ])
}

fn ok_result() -> Value {
    json!({"ok": true, "status_code": 200, "latency_ms": 143, "region": "fsn", "error": null})
}

fn failed_result() -> Value {
    json!({"ok": false, "status_code": 502, "latency_ms": 80, "region": "fsn", "error": "502 Bad Gateway"})
}

#[test]
fn maintenance_start_for_a_while() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/maintenance_windows", |_, _| (201, json!({"data": window()})));
    let (code, out, _) = world.run(&["maintenance", "start", "--sites", "yourbakery.com,shop.yourbakery.com", "--for", "15m", "--title", "Deploy", "--message", "Back soon"]);
    assert_eq!(code, 0);
    assert!(out.contains("Maintenance Deploy (mw_7) is on until"));
    assert!(out.contains("pingerer maintenance end mw_7"));
    assert_eq!(world.api.last().json(), json!({"title": "Deploy", "sites": ["yourbakery.com", "shop.yourbakery.com"], "duration": 900, "message": "Back soon", "notify_subscribers_before": false}));
}

#[test]
fn maintenance_start_prints_json_for_scripts() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/maintenance_windows", |_, _| (201, json!({"data": window()})));
    let (code, out, _) = world.run(&["maintenance", "start", "--site", "yourbakery.com", "--for", "1h", "--notify", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["id"], "mw_7");
    assert_eq!(world.api.last().json(), json!({"title": "Maintenance", "sites": ["yourbakery.com"], "duration": 3600}));
}

#[test]
fn maintenance_start_needs_sites_and_a_duration() {
    let world = World::new();
    world.logged_in();
    let (code, _, err) = world.run(&["maintenance", "start", "--for", "15m"]);
    assert_eq!(code, 2);
    assert!(err.contains("Which sites?"));
    let (code, _, err) = world.run(&["maintenance", "start", "--sites", "yourbakery.com"]);
    assert_eq!(code, 2);
    assert!(err.contains("--for 15m"));
}

#[test]
fn maintenance_end_by_id_and_by_title() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/maintenance_windows/mw_7", |_, _| ok(json!({"data": window()})));
    world.api.on("GET", "/api/v1/maintenance_windows", |_, _| {
        let mut scheduled = window();
        scheduled["id"] = json!("mw_6");
        scheduled["state"] = json!("scheduled");
        ok(json!({"data": [scheduled, window()], "meta": {}}))
    });
    world.api.on("POST", "/api/v1/maintenance_windows/mw_7/end", |_, _| {
        let mut done = window();
        done["state"] = json!("done");
        ok(json!({"data": done}))
    });
    let (code, out, _) = world.run(&["maintenance", "end", "mw_7"]);
    assert_eq!(code, 0);
    assert!(out.contains("Ended Deploy (done)."));
    let (code, _, _) = world.run(&["maintenance", "end", "deploy"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().path(), "/api/v1/maintenance_windows/mw_7/end", "the one in progress");
}

#[test]
fn check_runs_a_sites_uptime_monitors_once() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/monitors", |_, _| ok(json!({"data": monitors(), "meta": {}})));
    world.api.on("POST", "/api/v1/monitors/mon_1/test", |_, _| ok(json!({"data": ok_result()})));
    let (code, out, _) = world.run(&["check", "yourbakery.com", "--region", "iad"]);
    assert_eq!(code, 0);
    assert!(out.contains("Uptime") && out.contains("143 ms"));
    let tests: Vec<_> = world.api.all().into_iter().filter(|it| it.path().ends_with("/test")).collect();
    assert_eq!(tests.iter().map(|it| it.path()).collect::<Vec<_>>(), vec!["/api/v1/monitors/mon_1/test"], "only active uptime monitors");
    assert_eq!(tests[0].json(), json!({"region": "iad"}));
    assert_eq!(world.api.all()[0].params()["site_id"], "yourbakery.com");
}

#[test]
fn check_exits_3_when_a_check_fails() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/monitors/mon_1", |_, _| ok(json!({"data": monitors()[0]})));
    world.api.on("POST", "/api/v1/monitors/mon_1/test", |_, _| ok(json!({"data": failed_result()})));
    let (code, out, _) = world.run(&["check", "mon_1", "--json"]);
    assert_eq!(code, 3);
    let oks: Vec<Value> = serde_json::from_str::<Vec<Value>>(&out).unwrap().iter().map(|it| it["ok"].clone()).collect();
    assert_eq!(oks, vec![json!(false)]);
}

#[test]
fn check_needs_something_with_uptime_monitors() {
    let world = World::new();
    world.logged_in();
    let (code, _, err) = world.run(&["check"]);
    assert_eq!(code, 2);
    assert!(err.contains("What should be checked?"));

    world.api.on("GET", "/api/v1/monitors", |_, _| ok(json!({"data": [monitors()[1]], "meta": {}})));
    let (code, _, err) = world.run(&["check", "yourbakery.com"]);
    assert_eq!(code, 1);
    assert!(err.contains("no uptime monitors"));

    world.api.on("GET", "/api/v1/monitors", |_, _| ok(json!({"data": [monitors()[2]], "meta": {}})));
    let (code, _, err) = world.run(&["check", "yourbakery.com"]);
    assert_eq!(code, 1);
    assert!(err.contains("paused"));
}

#[test]
fn wait_until_up_checks_until_it_passes() {
    let world = World::new();
    world.logged_in();
    let runs = Rc::new(Cell::new(0));
    world.api.on("GET", "/api/v1/monitors", |_, _| ok(json!({"data": monitors(), "meta": {}})));
    let counted = runs.clone();
    world.api.on("POST", "/api/v1/monitors/mon_1/test", move |_, _| {
        counted.set(counted.get() + 1);
        ok(json!({"data": if counted.get() < 3 { failed_result() } else { ok_result() }}))
    });
    let (code, out, err) = world.run(&["wait-until-up", "yourbakery.com", "--interval", "0", "--timeout", "1m"]);
    assert_eq!(code, 0);
    assert_eq!(runs.get(), 3);
    assert!(out.contains("Up after 3 checks"), "{out}");
    assert!(err.contains("Not up yet (Uptime)"));
}

#[test]
fn wait_until_up_gives_up_after_the_timeout() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/monitors/mon_1", |_, _| ok(json!({"data": monitors()[0]})));
    world.api.on("POST", "/api/v1/monitors/mon_1/test", |_, _| ok(json!({"data": failed_result()})));
    let (code, out, _) = world.run(&["wait-until-up", "mon_1", "--timeout", "0s"]);
    assert_eq!(code, 3);
    assert!(out.contains("Still down after 0s"));
    assert!(out.contains("502 Bad Gateway"));
    let (code, _, err) = world.run(&["wait-until-up", "mon_1", "--timeout", "soon"]);
    assert_eq!(code, 2);
    assert!(err.contains("isn't a duration"));
}

#[test]
fn falls_back_to_the_monitors_status_without_test_runs() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/monitors/mon_1", |_, _| ok(json!({"data": monitors()[0]})));
    world.api.on("POST", "/api/v1/monitors/mon_1/test", |_, _| (501, json!({"error": "not_available", "message": "Test runs aren't available yet."})));
    let (code, _, _) = world.run(&["check", "mon_1"]);
    assert_eq!(code, 0);
}

fn scheduled() -> Value {
    json!({"id": "mw_1", "title": "Server upgrade", "state": "scheduled", "all_sites": false,
        "sites": [{"id": "site_1", "host": "shop.yourbakery.com"}], "recurrence": "none", "days": [],
        "starts_at": "2030-10-14T06:00:00Z", "ends_at": "2030-10-14T06:30:00Z",
        "next_occurrence": {"starts_at": "2030-10-14T06:00:00Z", "ends_at": "2030-10-14T06:30:00Z"}})
}

#[test]
fn maintenance_list_and_empty() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/maintenance_windows", |_, _| ok(json!({"data": [scheduled()], "meta": {}})));
    let (code, out, _) = world.run(&["maintenance", "list", "--upcoming"]);
    assert_eq!(code, 0);
    assert!(out.contains("Server upgrade") && out.contains("shop.yourbakery.com"));
    assert_eq!(world.api.last().params()["state"], "upcoming");

    world.api.on("GET", "/api/v1/maintenance_windows", |_, _| ok(json!({"data": [], "meta": {}})));
    let (code, out, _) = world.run(&["maintenance"]);
    assert_eq!(code, 0);
    assert!(out.contains("No maintenance windows"));
}

#[test]
fn maintenance_add_with_a_duration_in_a_time_zone() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/maintenance_windows", |_, _| (201, json!({"data": scheduled()})));
    let (code, out, _) = world.run(&["maintenance", "add", "Server", "upgrade", "--start", "2030-10-14 02:00", "--for", "30m",
        "--time-zone", "America/Toronto", "--sites", "shop.yourbakery.com", "--no-notify"]);
    assert_eq!(code, 0);
    assert!(out.contains("Scheduled Server upgrade (mw_1)"));
    assert_eq!(world.api.last().json(), json!({"title": "Server upgrade", "starts_at": "2030-10-14T02:00:00", "ends_at": "2030-10-14T02:30:00",
        "time_zone": "America/Toronto", "sites": ["shop.yourbakery.com"], "notify_subscribers_before": false}));
}

#[test]
fn maintenance_add_weekly_for_all_sites() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/maintenance_windows", |_, _| {
        let mut all = scheduled();
        all["all_sites"] = json!(true);
        (201, json!({"data": all}))
    });
    let (code, _, _) = world.run(&["maintenance", "add", "Backups", "--start", "2030-10-13T03:00:00Z", "--end", "2030-10-13T03:15:00Z", "--all-sites", "--weekly", "--days", "sun,wed"]);
    assert_eq!(code, 0);
    let body = world.api.last().json();
    assert_eq!(body["recurrence"], "weekly");
    assert_eq!(body["days"], json!(["sun", "wed"]));
    assert_eq!(body["all_sites"], json!(true));
    let starts = chrono::DateTime::parse_from_rfc3339(body["starts_at"].as_str().unwrap()).unwrap();
    assert_eq!(starts, chrono::DateTime::parse_from_rfc3339("2030-10-13T03:00:00Z").unwrap());
}

#[test]
fn maintenance_add_in_local_time_with_a_duration() {
    let world = World::new();
    world.logged_in();
    world.api.on("POST", "/api/v1/maintenance_windows", |_, _| (201, json!({"data": scheduled()})));
    world.run(&["maintenance", "add", "Upgrade", "--start", "2030-10-14 02:00", "--for", "2h", "--all-sites"]);
    let body = world.api.last().json();
    let starts = chrono::DateTime::parse_from_rfc3339(body["starts_at"].as_str().unwrap()).unwrap();
    let ends = chrono::DateTime::parse_from_rfc3339(body["ends_at"].as_str().unwrap()).unwrap();
    assert_eq!((ends - starts).num_seconds(), 7200);
    assert_eq!(starts.format("%H:%M").to_string(), "02:00");
}

#[test]
fn maintenance_add_needs_a_start_an_end_and_sites() {
    let world = World::new();
    world.logged_in();
    let (code, _, err) = world.run(&["maintenance", "add", "Upgrade", "--start", "2030-10-14 02:00", "--all-sites"]);
    assert_eq!(code, 2);
    assert!(err.contains("--for"));
    let (code, _, err) = world.run(&["maintenance", "add", "Upgrade", "--start", "2030-10-14 02:00", "--for", "1h"]);
    assert_eq!(code, 2);
    assert!(err.contains("--all-sites"));
    let (code, _, err) = world.run(&["maintenance", "add", "Upgrade", "--start", "next tuesday", "--for", "1h", "--all-sites"]);
    assert_eq!(code, 2);
    assert!(err.contains("isn't a time"));
}

#[test]
fn maintenance_cancel_by_title_and_unknown() {
    let world = World::new();
    world.logged_in();
    world.api.on("GET", "/api/v1/maintenance_windows", |_, _| ok(json!({"data": [scheduled()], "meta": {}})));
    world.api.on("POST", "/api/v1/maintenance_windows/mw_1/cancel", |_, _| {
        let mut cancelled = scheduled();
        cancelled["state"] = json!("cancelled");
        ok(json!({"data": cancelled}))
    });
    let (code, out, _) = world.run(&["maintenance", "cancel", "server upgrade", "--yes"]);
    assert_eq!(code, 0);
    assert!(out.contains("Cancelled Server upgrade."));
    assert_eq!(world.api.last().path(), "/api/v1/maintenance_windows/mw_1/cancel");

    world.api.on("GET", "/api/v1/maintenance_windows", |_, _| ok(json!({"data": [], "meta": {}})));
    let (code, _, err) = world.run(&["maintenance", "cancel", "nothing", "--yes"]);
    assert_eq!(code, 1);
    assert!(err.contains("No maintenance window nothing"));
}
