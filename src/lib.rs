//! The Pingerer command line tool. See README.md, or run `pingerer help`.
//!
//! A port of the Ruby gem (cli/), with the same commands, options, output,
//! exit codes, config file and environment variables. It talks to the API
//! described in public/openapi.yaml.

pub mod cli;
pub mod client;
pub mod config;
pub mod errors;
pub mod options;
pub mod output;
pub mod values;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_URL: &str = "https://pingerer.com";
