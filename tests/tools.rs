//! The Ruby gem's health_and_tools_test.rb and status_pages_test.rb: site
//! health, link previews, Internet weather, quiet hours, a domain's renewal
//! date, host trouble on incidents and status pages.

mod common;

use common::{ok, World};
use serde_json::{json, Value};
use std::cell::Cell;
use std::rc::Rc;

fn health(score: i64, running: bool, critical: i64) -> Value {
    json!({"data": {
        "site_id": "site_1", "score": score, "score_change": -6, "checked_at": "2026-10-30T10:00:00Z", "running": running,
        "counts": {"critical": critical, "warning": 0, "notice": 1, "pass": 1, "unknown": 0},
        "areas": [
            {"key": "dns", "name": "DNS", "findings": [
                {"key": "dns.dangling_cname", "severity": if critical > 0 { "critical" } else { "pass" }, "title": "www points nowhere", "detail": "The CNAME's target doesn't exist.", "fix": "Remove the record."}
            ]},
            {"key": "sharing", "name": "Link previews", "findings": [
                {"key": "sharing.image", "severity": "notice", "title": "No share picture", "fix": "Add an og:image tag."},
                {"key": "sharing.title", "severity": "pass", "title": "Has a title"}
            ]}
        ]
    }})
}

fn world() -> World {
    let world = World::new();
    world.logged_in();
    world
}

#[test]
fn health_shows_the_score_and_what_to_fix_and_exits_3_on_a_critical_problem() {
    let world = world();
    world.api.on("GET", "/api/v1/sites/yourbakery.com/health", |_, _| ok(health(72, false, 1)));
    let (code, out, _) = world.run(&["health", "yourbakery.com"]);
    assert_eq!(code, 3);
    assert!(out.contains("72/100 (-6 since the last check)"), "{out}");
    assert!(out.contains("1 critical · 0 warning · 1 notice"));
    assert!(out.contains("www points nowhere (DNS)"));
    assert!(out.contains("Fix: Remove the record."));
    assert!(out.contains("No share picture (Link previews)"));
    assert!(!out.contains("Has a title"));
    let (_, out, _) = world.run(&["health", "yourbakery.com", "--all"]);
    assert!(out.contains("Has a title"));
}

#[test]
fn health_without_problems_and_before_the_first_check() {
    let world = world();
    world.api.on("GET", "/api/v1/sites/yourbakery.com/health", |_, _| ok(health(90, false, 0)));
    assert_eq!(world.run(&["health", "yourbakery.com"]).0, 0);

    world.api.on("GET", r"^/api/v1/sites/.+/health$", |_, _| ok(json!({"data": {"score": null, "running": false, "counts": {}, "areas": []}})));
    let (code, out, _) = world.run(&["health", "yourbakery.com"]);
    assert_eq!(code, 0);
    assert!(out.contains("hasn't had a health check yet"));
}

#[test]
fn health_now_starts_a_check_and_waits_for_it() {
    let world = world();
    let polls = Rc::new(Cell::new(0));
    world.api.on("POST", "/api/v1/sites/yourbakery.com/health", |_, _| (202, health(72, true, 1)));
    let counted = polls.clone();
    world.api.on("GET", "/api/v1/sites/yourbakery.com/health", move |_, _| {
        counted.set(counted.get() + 1);
        ok(if counted.get() < 2 { health(72, true, 1) } else { health(97, false, 0) })
    });
    let (code, out, err) = world.run(&["health", "yourbakery.com", "--now", "--interval", "0"]);
    assert_eq!(code, 0);
    assert!(err.contains("Checking"));
    assert!(out.contains("97/100"));
    assert_eq!(polls.get(), 2);
}

#[test]
fn health_now_shows_the_last_one_when_it_ran_a_moment_ago() {
    let world = world();
    world.api.on("POST", "/api/v1/sites/yourbakery.com/health", |_, _| (429, json!({"error": "too_soon", "message": "A health check ran a moment ago. You can check again in 8 minutes."})));
    world.api.on("GET", "/api/v1/sites/yourbakery.com/health", |_, _| ok(health(72, false, 0)));
    let (code, out, err) = world.run(&["health", "yourbakery.com", "--now"]);
    assert_eq!(code, 0);
    assert!(err.contains("check again in 8 minutes"));
    assert!(out.contains("72/100"));
}

#[test]
fn link_preview_shows_the_preview_and_exits_3_when_something_is_broken() {
    let world = world();
    world.api.on("GET", "/api/v1/link_preview", |_, _| ok(json!({"data": {"address": "yourbakery.com/menu", "url": "https://yourbakery.com/menu", "status": "fail",
        "checks": [{"status": "pass", "title": "Has a title"}, {"status": "fail", "title": "Share picture is broken", "detail": "It answers 404.", "fix": "Upload it again."}],
        "preview": {"title": "Our menu", "description": null, "image": null}}})));
    let (code, out, _) = world.run(&["link-preview", "yourbakery.com/menu"]);
    assert_eq!(code, 3);
    assert_eq!(world.api.last().params(), std::collections::HashMap::from([("url".to_string(), "yourbakery.com/menu".to_string())]));
    assert!(out.contains("Our menu") && out.contains("Share picture is broken") && out.contains("Fix: Upload it again."));
    assert!(!out.contains("Fix: \n"));
    assert!(out.contains("Picture") && out.contains("none"));

    let (code, _, err) = world.run(&["link-preview"]);
    assert_eq!(code, 2);
    assert!(err.contains("Which page?"));
}

#[test]
fn weather_lists_companies_and_outages() {
    let world = world();
    world.api.on("GET", "/api/v1/internet_weather", |_, _| ok(json!({"data": {"trouble": true, "companies": [
        {"name": "Fastly", "slug": "fastly", "role": "CDN", "up_percent": 25, "trouble": true},
        {"name": "Hetzner", "slug": "hetzner", "role": "hosting company", "up_percent": 99, "trouble": false}
    ], "outages": [{"name": "Fastly", "started_at": "2026-10-30T10:00:00Z", "ongoing": true, "duration_seconds": 1200}]}})));
    let (code, out, _) = world.run(&["weather"]);
    assert_eq!(code, 0);
    assert!(out.contains("Having trouble now: Fastly"));
    assert!(regex::Regex::new(r"Hetzner\s+hosting company\s+99%").unwrap().is_match(&out));
    assert!(out.contains("still going (20m so far)"), "{out}");
}

#[test]
fn weather_for_one_company_exits_3_when_it_has_trouble() {
    let world = world();
    world.api.on("GET", "/api/v1/internet_weather/cloudflare", |_, _| ok(json!({"data": {"name": "Cloudflare", "state": "trouble", "listings": [{"role": "CDN", "up_percent": 40}],
        "status_page_url": "https://www.cloudflarestatus.com", "url": "https://pingerer.test/internet-weather/cloudflare", "outages": []}})));
    let (code, out, _) = world.run(&["weather", "cloudflare"]);
    assert_eq!(code, 3);
    assert!(out.contains("Looks like trouble") && out.contains("40%") && out.contains("https://www.cloudflarestatus.com"));
}

#[test]
fn quiet_hours_show_and_set() {
    let world = world();
    let quiet = json!({"enabled": true, "summary": "10:00 PM – 7:00 AM and all weekend, Asia/Kuwait", "weekend": null, "weekend_days": ["friday", "saturday"], "quiet_now": false});
    let shown = quiet.clone();
    world.api.on("GET", "/api/v1/quiet_hours", move |_, _| ok(json!({"data": shown})));
    world.api.on("PATCH", "/api/v1/quiet_hours", move |_, _| ok(json!({"data": quiet})));

    let (code, out, _) = world.run(&["quiet-hours"]);
    assert_eq!(code, 0);
    assert!(out.contains("10:00 PM – 7:00 AM and all weekend"));
    assert!(out.contains("Friday and Saturday (from your time zone)"));

    let (code, _, _) = world.run(&["quiet-hours", "set", "--on", "--from", "22:00", "--to", "07:00", "--time-zone", "Asia/Kuwait", "--weekends", "--weekend", "auto"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"enabled": true, "starts": "22:00", "ends": "07:00", "time_zone": "Asia/Kuwait", "weekends": true, "weekend": ""}));

    world.run(&["quiet-hours", "set", "--off", "--no-weekends", "--weekend", "fri"]);
    assert_eq!(world.api.last().json(), json!({"enabled": false, "weekends": false, "weekend": "fri"}));

    let (code, _, err) = world.run(&["quiet-hours", "set", "--weekend", "mon"]);
    assert_eq!(code, 2);
    assert!(err.contains("--weekend takes"));
    let (code, _, err) = world.run(&["quiet-hours", "set"]);
    assert_eq!(code, 2);
    assert!(err.contains("Nothing to change"));
}

#[test]
fn monitors_set_a_domain_renewal_date() {
    let world = world();
    world.api.on("PATCH", "/api/v1/monitors/mon_1", |request, _| ok(json!({"data": {"id": "mon_1", "name": "Domain renewal", "settings": request.json()["settings"]}})));
    let (code, out, _) = world.run(&["monitors", "set", "mon_1", "--renews-on", "2027-03-02", "--warn-days", "60,14"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json(), json!({"settings": {"renews_on": "2027-03-02", "warn_days": [60, 14]}}));
    assert!(out.contains("Renews on 2027-03-02."));

    world.run(&["monitors", "set", "mon_1", "--renews-on", "none"]);
    assert_eq!(world.api.last().json(), json!({"settings": {"renews_on": null}}));

    let (code, _, err) = world.run(&["monitors", "set", "mon_1", "--renews-on", "next spring"]);
    assert_eq!(code, 2);
    assert!(err.contains("a date like 2027-03-02"));
}

#[test]
fn incident_show_says_when_its_probably_the_host() {
    let world = world();
    world.api.on("GET", "/api/v1/incidents/INC-0042", |_, _| ok(json!({"data": {"reference": "INC-0042", "kind": "down", "state": "open",
        "provider_trouble": {"kind": "cdn", "name": "Cloudflare"}, "events": [{"created_at": "2026-10-30T10:00:00Z", "kind": "opened", "region": "yyz", "message": "502"}]}})));
    let (_, out, _) = world.run(&["incidents", "show", "INC-0042"]);
    assert!(out.contains("Probably not you: lots of other sites on Cloudflare went down at the same time"));
    assert!(out.contains("Timeline") && out.contains("yyz"));
}

#[test]
fn status_pages_list_and_show() {
    let world = world();
    world.api.on("GET", "/api/v1/status_pages", |_, _| ok(json!({"data": [{"name": "Your Bakery", "url": "https://bakery.status.pingerer.com", "published": true, "slug": "bakery"}], "meta": {}})));
    let (code, out, _) = world.run(&["status-pages", "list"]);
    assert_eq!(code, 0);
    assert!(out.contains("https://bakery.status.pingerer.com"));

    world.api.on("GET", "/api/v1/status_pages/bakery", |_, _| ok(json!({"data": {"name": "Your Bakery", "status": "partial_outage", "components": [{"name": "Online shop", "status": "partial_outage"}],
        "badges": {"status": "https://bakery.status.test/badge/status.svg", "monitored": "https://bakery.status.test/badge/monitored.svg", "link": "https://bakery.status.test/"}}})));
    let (code, out, _) = world.run(&["status-pages", "show", "bakery"]);
    assert_eq!(code, 0);
    assert!(out.contains("Online shop") && out.contains("partial outage"));
    assert!(out.contains("https://bakery.status.test/badge/monitored.svg"));
    assert!(out.contains("Each one links to https://bakery.status.test/."));
}

#[test]
fn updates_post_and_follow_up() {
    let world = world();
    world.api.on("POST", "/api/v1/status_pages/bakery/updates", |request, _| {
        let mut data = request.json();
        data["id"] = json!("su_1");
        (201, json!({"data": data}))
    });
    let (code, out, _) = world.run(&["updates", "post", "bakery", "--state", "investigating", "--title", "Checkout errors", "--message", "On it", "--component", "Online shop=partial_outage"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Posted"));
    assert_eq!(world.api.last().json(), json!({"state": "investigating", "title": "Checkout errors", "message": "On it", "components": {"Online shop": "partial_outage"}}));

    let (code, _, _) = world.run(&["updates", "post", "bakery", "--state", "resolved", "--message", "Fixed", "--follow-up", "su_1"]);
    assert_eq!(code, 0);
    assert_eq!(world.api.last().json()["thread_id"], "su_1");
}

#[test]
fn updates_post_usage_errors_and_list() {
    let world = world();
    let (code, _, err) = world.run(&["updates", "post", "bakery", "--state", "nope", "--title", "x"]);
    assert_eq!(code, 2);
    assert!(err.contains("--state"));
    let (code, _, err) = world.run(&["updates", "post", "bakery", "--state", "investigating"]);
    assert_eq!(code, 2);
    assert!(err.contains("--title"));
    let (code, _, _) = world.run(&["updates", "post", "bakery", "--state", "investigating", "--title", "x", "--component", "Shop=broken"]);
    assert_eq!(code, 2);

    world.api.on("GET", "/api/v1/status_pages/bakery/updates", |_, _| ok(json!({"data": [{"published_at": "2026-10-07T10:00:00Z", "state": "investigating", "title": "Checkout errors", "source": "api", "id": "su_1"}], "meta": {}})));
    let (code, out, _) = world.run(&["updates", "list", "bakery", "--open"]);
    assert_eq!(code, 0);
    assert!(out.contains("Checkout errors"));
    assert_eq!(world.api.last().params()["open"], "true");
}
