use std::error::Error;
pub use thiserror::Error;
use url::Url;

pub mod fetcher;

#[derive(Error, Debug)]
pub enum GopherError {
    #[error("{0}")]
    AgentError(Box<dyn Error + Send + 'static>),
    #[error("Timeout on: {0}")]
    Timeout(Url),
    #[error("Too many stuff happening: {0}")]
    GopherStuff(String),
    #[error("Read error: {0}")]
    ReadError(String),
    #[error("Connection error: {0}")]
    ConnectionError(String),
    #[error("Invalid gopher URL: {0}")]
    InvalidUrl(String),
}
