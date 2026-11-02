//! Talks to the Pingerer API (/api/v1): bearer token, JSON both ways, the
//! account in the Pingerer-Account header. Error responses become
//! Error::Api; a server that can't be reached, Error::Failed.
//!
//! The transport is anything that makes one HTTP request: ureq by default,
//! a fake in tests.

use crate::errors::{failed, ApiError, Error, Result};
use crate::VERSION;
use serde_json::{json, Value};
use std::rc::Rc;
use std::time::Duration;

pub fn user_agent() -> String {
    format!("pingerer-cli/{VERSION} (rust)")
}

pub trait Transport {
    /// One request; the response's status and body.
    fn call(&self, method: &str, url: &str, headers: &[(String, String)], body: Option<&str>) -> Result<(u16, String)>;
}

/// HTTP through ureq, one request per call. TLS certificates are verified,
/// and a token is only sent in the clear to this machine (localhost,
/// *.localhost, *.test), for running Pingerer locally.
pub struct HttpTransport {
    agent: ureq::Agent,
}

impl Default for HttpTransport {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .build();
        HttpTransport { agent: config.into() }
    }
}

pub fn local(host: &str) -> bool {
    let host = host.to_lowercase();
    ["localhost", "127.0.0.1", "::1", "[::1]"].contains(&host.as_str()) || host.ends_with(".localhost") || host.ends_with(".test")
}

/// The scheme and host of a URL, without parsing more than that.
pub fn scheme_and_host(url: &str) -> (String, String) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or(authority).to_string()
    } else {
        authority.split(':').next().unwrap_or("").to_string()
    };
    (scheme.to_lowercase(), host)
}

impl Transport for HttpTransport {
    fn call(&self, method: &str, url: &str, headers: &[(String, String)], body: Option<&str>) -> Result<(u16, String)> {
        let (scheme, host) = scheme_and_host(url);
        if scheme != "http" && scheme != "https" {
            return failed(format!("{scheme}:// isn't supported. Use an https:// address."));
        }
        if scheme == "http" && headers.iter().any(|(name, _)| name == "Authorization") && !local(&host) {
            return failed(format!("Refusing to send your token over plain http to {host}. Use https://{host}."));
        }

        let unreachable = |error: ureq::Error| Error::Failed(format!("Couldn't reach {scheme}://{host}: {error}"));
        let mut request = ureq::http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let mut response = match body {
            Some(body) => self.agent.run(request.body(body.to_string()).map_err(|error| Error::Failed(error.to_string()))?),
            None => self.agent.run(request.body(()).map_err(|error| Error::Failed(error.to_string()))?),
        }
        .map_err(unreachable)?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        Ok((status, text))
    }
}

pub struct Client {
    pub url: String,
    token: Option<String>,
    pub account: Option<String>,
    transport: Rc<dyn Transport>,
}

impl Client {
    pub fn new(url: &str, token: Option<String>, transport: Rc<dyn Transport>) -> Self {
        Client { url: url.trim_end_matches('/').to_string(), token, account: None, transport }
    }

    pub fn get(&self, path: &str, params: &[(&str, Option<String>)]) -> Result<Value> {
        self.request("GET", path, params, None)
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request("POST", path, &[], Some(body))
    }

    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.request("PATCH", path, &[], Some(body))
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        self.request("DELETE", path, &[], None)
    }

    /// Every page of a list's data, up to max_pages (50).
    pub fn get_all(&self, path: &str, params: &[(&str, Option<String>)]) -> Result<Vec<Value>> {
        let mut page = 1;
        let mut items = vec![];
        loop {
            let mut query: Vec<(&str, Option<String>)> = params.to_vec();
            query.push(("page", Some(page.to_string())));
            query.push(("per_page", Some("100".into())));
            let response = self.get(path, &query)?;
            items.extend(crate::values::list(crate::values::at(&response, "data")));
            match crate::values::dig(&response, &["meta", "next_page"]).as_i64() {
                Some(next) if next <= 50 => page = next,
                _ => break,
            }
        }
        Ok(items)
    }

    pub fn request(&self, method: &str, path: &str, params: &[(&str, Option<String>)], body: Option<Value>) -> Result<Value> {
        let lower = self.url.to_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return failed(format!("{} isn't an http(s) address. Use one like https://pingerer.com.", self.url));
        }
        let mut url = format!("{}/{}", self.url, path.trim_start_matches('/'));
        let query: Vec<String> = params.iter().filter_map(|(key, value)| value.as_ref().filter(|it| !it.is_empty()).map(|value| format!("{}={}", form(key), form(value)))).collect();
        if !query.is_empty() {
            url = format!("{url}?{}", query.join("&"));
        }
        let payload = body.map(|body| serde_json::to_string(&compact(body)).unwrap_or_default());
        let (status, raw) = self.transport.call(method, &url, &self.headers(payload.is_some()), payload.as_deref())?;
        let parsed = parse(&raw);
        if !(200..300).contains(&status) {
            return Err(Error::Api(Box::new(error_for(status, &parsed))));
        }
        Ok(parsed)
    }

    fn headers(&self, json_body: bool) -> Vec<(String, String)> {
        let mut headers = vec![("Accept".to_string(), "application/json".to_string()), ("User-Agent".to_string(), user_agent())];
        if let Some(token) = &self.token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(account) = &self.account {
            headers.push(("Pingerer-Account".into(), account.clone()));
        }
        if json_body {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        headers
    }
}

/// The top level's nil values left out (Ruby's Hash#compact).
fn compact(body: Value) -> Value {
    match body {
        Value::Object(map) => Value::Object(map.into_iter().filter(|(_, value)| !value.is_null()).collect()),
        other => other,
    }
}

fn parse(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|_| json!({"message": raw.chars().take(200).collect::<String>()}))
}

fn error_for(status: u16, parsed: &Value) -> ApiError {
    let code = parsed.get("error").and_then(Value::as_str).map(String::from).unwrap_or_else(|| default_code(status));
    let message = parsed.get("message").and_then(Value::as_str).map(String::from).unwrap_or_else(|| default_message(status));
    ApiError { status, code, message, details: parsed.get("details").cloned().filter(|it| !it.is_null()) }
}

fn default_code(status: u16) -> String {
    match status {
        401 => "unauthorized".into(),
        403 => "forbidden".into(),
        404 => "not_found".into(),
        422 => "validation_failed".into(),
        429 => "rate_limited".into(),
        other => format!("http_{other}"),
    }
}

fn default_message(status: u16) -> String {
    match status {
        401 => "Not signed in, or the token isn't valid. Run `pingerer login`.".into(),
        403 => "Not allowed.".into(),
        404 => "Not found.".into(),
        500..=599 => format!("Pingerer had a problem ({status}). Try again in a moment."),
        other => format!("The request failed ({other})."),
    }
}

/// URI.encode_www_form_component: letters, digits and *-._ as they are,
/// spaces as +, everything else %XX.
pub fn form(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A path segment: like form, with %20 for spaces.
pub fn escape(text: &str) -> String {
    form(text).replace('+', "%20")
}
