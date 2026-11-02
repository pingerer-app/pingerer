//! Answers the CLI's requests from canned routes, like a tiny Pingerer (the
//! Ruby gem's test/test_helper.rb FakeAPI):
//!
//!   api.on("GET", "/api/v1/status", |_| ok(json!({"data": ...})));
//!   api.on("POST", r"^/api/v1/sites/(.+)/pause$", |_| (422, json!({"error": "plan_limit"})));
//!
//! A path starting with ^ is a regular expression.

#![allow(dead_code)]

use pingerer::cli::{run, Io};
use pingerer::client::Transport;
use pingerer::errors::Result;
use regex::Regex;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl Request {
    pub fn path(&self) -> String {
        let after = self.url.split_once("://").map_or(self.url.as_str(), |(_, rest)| rest);
        let path = after.find('/').map_or("/", |at| &after[at..]);
        path.split('?').next().unwrap_or("/").to_string()
    }

    pub fn host(&self) -> String {
        pingerer::client::scheme_and_host(&self.url).1
    }

    pub fn params(&self) -> HashMap<String, String> {
        let query = self.url.split_once('?').map_or("", |(_, query)| query);
        query.split('&').filter(|it| !it.is_empty()).map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        }).collect()
    }

    pub fn json(&self) -> Value {
        self.body.as_deref().map(|body| serde_json::from_str(body).unwrap()).unwrap_or(json!({}))
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.clone())
    }
}

fn decode(text: &str) -> String {
    let text = text.replace('+', " ");
    let mut out = vec![];
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            out.push(u8::from_str_radix(&text[index + 1..index + 3], 16).unwrap());
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

type Handler = Box<dyn Fn(&Request, &[String]) -> (u16, Value)>;

#[derive(Default)]
pub struct FakeApi {
    routes: RefCell<Vec<(String, String, Handler)>>,
    pub requests: RefCell<Vec<Request>>,
}

pub fn ok(data: Value) -> (u16, Value) {
    (200, data)
}

impl FakeApi {
    pub fn on(&self, method: &str, path: &str, handler: impl Fn(&Request, &[String]) -> (u16, Value) + 'static) {
        self.routes.borrow_mut().push((method.into(), path.into(), Box::new(handler)));
    }

    pub fn last(&self) -> Request {
        self.requests.borrow().last().cloned().expect("a request")
    }

    pub fn all(&self) -> Vec<Request> {
        self.requests.borrow().clone()
    }
}

impl Transport for FakeApi {
    fn call(&self, method: &str, url: &str, headers: &[(String, String)], body: Option<&str>) -> Result<(u16, String)> {
        let request = Request { method: method.into(), url: url.into(), headers: headers.to_vec(), body: body.map(String::from) };
        self.requests.borrow_mut().push(request.clone());
        let path = request.path();
        for (route_method, route_path, handler) in self.routes.borrow().iter().rev() {
            if route_method != method {
                continue;
            }
            let captures: Option<Vec<String>> = if route_path.starts_with('^') {
                Regex::new(route_path).unwrap().captures(&path).map(|found| found.iter().map(|it| it.map_or(String::new(), |it| it.as_str().to_string())).collect())
            } else {
                (route_path == &path).then(|| vec![path.clone()])
            };
            if let Some(captures) = captures {
                let (status, data) = handler(&request, &captures);
                return Ok((status, if data.is_null() { String::new() } else { data.to_string() }));
            }
        }
        Ok((404, json!({"error": "not_found", "message": format!("No route for {method} {path}")}).to_string()))
    }
}

#[derive(Clone, Default)]
pub struct Buffer(Rc<RefCell<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buffer {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).unwrap()
    }
}

/// A config directory of its own and a fake API, per test.
pub struct World {
    pub dir: PathBuf,
    pub api: Rc<FakeApi>,
    pub env: HashMap<String, String>,
    pub sleeps: Rc<RefCell<Vec<f64>>>,
}

impl World {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("pingerer-cli-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("pingerer").join("config.json");
        let env = HashMap::from([("PINGERER_CONFIG".to_string(), config.display().to_string())]);
        World { dir, api: Rc::new(FakeApi::default()), env, sleeps: Rc::default() }
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("pingerer").join("config.json")
    }

    pub fn config(&self) -> pingerer::config::Config {
        pingerer::config::Config::new(self.config_path(), self.env.clone())
    }

    pub fn logged_in(&self) -> &Self {
        self.config().update(vec![("token", Some("tok_test".into())), ("url", Some("https://pingerer.test".into()))]).unwrap();
        self
    }

    /// Runs the CLI; (exit code, stdout, stderr).
    pub fn run(&self, argv: &[&str]) -> (i32, String, String) {
        self.run_with(argv, "", &[])
    }

    pub fn run_with(&self, argv: &[&str], input: &str, env: &[(&str, &str)]) -> (i32, String, String) {
        let out = Buffer::default();
        let err = Buffer::default();
        let mut all_env = self.env.clone();
        all_env.extend(env.iter().map(|(key, value)| (key.to_string(), value.to_string())));
        let sleeps = self.sleeps.clone();
        let io = Io {
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            out_tty: false,
            input: Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
            input_tty: false,
            env: all_env,
            transport: self.api.clone(),
            sleeper: Box::new(move |seconds| sleeps.borrow_mut().push(seconds)),
        };
        let code = run(argv.iter().map(|it| it.to_string()).collect(), io);
        (code, out.text(), err.text())
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rand_suffix() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::time::SystemTime::now())
}

pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
