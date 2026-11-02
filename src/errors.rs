use serde_json::Value;

/// Why a command stopped. Each maps to an exit code (cli::run).
#[derive(Debug)]
pub enum Error {
    /// Wrong arguments or options: exit code 2.
    Usage(String),
    /// The API answered with an error ({error:, message:, details:}).
    Api(Box<ApiError>),
    /// Anything else, including a server that can't be reached: exit code 1.
    Failed(String),
    /// --help on a command: its help was printed, and it exits 0.
    HelpShown,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn usage<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Usage(message.into()))
}

pub fn failed<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Failed(message.into()))
}
