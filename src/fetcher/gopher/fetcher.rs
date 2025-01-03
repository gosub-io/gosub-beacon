use crate::fetcher::gopher::GopherError;
use gophermap::{GopherEntry as LibGopherEntry, ItemType};
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
use url::Url;

pub struct GopherRequest {
    url: Url,
}

impl GopherRequest {
    pub fn new(url: Url) -> Self {
        Self { url }
    }
}

pub struct GopherEntry {
    pub item_type: ItemType,
    pub display_string: String,
    pub selector: String,
    pub host: String,
    pub port: u16,
}

pub struct GopherResponse {
    pub entries: Vec<GopherEntry>,
}

pub struct GopherFetcher {
    base_url: Url,
    // client: GopherRequestAgent,
    // middleware: Option<>
}

impl GopherFetcher {
    pub fn new(base_url: Url) -> Self {
        Self { base_url }
    }

    pub async fn fetch(&self, request: GopherRequest) -> Result<GopherResponse, GopherError> {
        let host = request.url.host_str().ok_or(GopherError::InvalidUrl(request.url.to_string()))?;
        let port = request.url.port().unwrap_or(70);
        let selector = request.url.path();

        // @todo: Async?
        let mut stream = TcpStream::connect((host, port)).map_err(|e| GopherError::ConnectionError(e.to_string()))?;
        stream
            .write_all(selector.as_bytes())
            .and_then(|_| stream.write_all(b"\r\n"))
            .map_err(|e| GopherError::ReadError(e.to_string()))?;

        let mut response = String::new();
        let mut reader = io::BufReader::new(stream);
        reader
            .read_to_string(&mut response)
            .map_err(|e| GopherError::ReadError(e.to_string()))?;

        let mut entries = Vec::new();
        for line in response.lines() {
            match LibGopherEntry::from(format!("{}\r\n", line).as_str()) {
                Some(entry) => {
                    entries.push(GopherEntry {
                        item_type: entry.item_type,
                        display_string: entry.display_string.to_string(),
                        selector: entry.selector.to_string(),
                        host: entry.host.to_string(),
                        port: entry.port,
                    });
                }
                None => {
                    dbg!("Invalid entry");
                }
            }
        }

        Ok(GopherResponse { entries })
    }
}
