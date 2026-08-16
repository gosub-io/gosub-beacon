use log::error;
use thiserror::Error;
use url::Url;

#[cfg(not(feature = "proto-http"))]
compile_error!("Feature 'proto-http' cannot be disabled.");

#[cfg(feature = "proto-ftp")]
mod ftp;
#[cfg(feature = "proto-gopher")]
mod gopher;
#[cfg(feature = "proto-http")]
mod http;

pub mod address_parser;
mod async_stream;

#[derive(Error, Debug)]
pub enum FetcherError {
    #[cfg(feature = "proto-http")]
    #[error("http error: {0}")]
    Http(#[from] http::HttpError),

    #[cfg(feature = "proto-ftp")]
    #[error("ftp error: {0}")]
    Ftp(#[from] ftp::FtpError),

    #[cfg(feature = "proto-gopher")]
    #[error("gopher error: {0}")]
    Gopher(#[from] gopher::GopherError),

    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

#[cfg(feature = "proto-http")]
pub use crate::fetcher::http::{response::HttpResponse, CompleteHttpFetcher, HttpBody, HttpMethod};

#[cfg(feature = "proto-ftp")]
pub use crate::fetcher::ftp::{fetcher::FtpFetcher, fetcher::FtpRequest, fetcher::FtpResponse};

#[cfg(feature = "proto-gopher")]
pub use crate::fetcher::gopher::{fetcher::GopherFetcher, fetcher::GopherRequest, fetcher::GopherResponse};
use crate::fetcher::http::request::HttpRequestBuilder;

enum Response {
    #[cfg(feature = "proto-http")]
    Http(HttpResponse),
    #[cfg(feature = "proto-ftp")]
    Ftp(FtpResponse),
    #[cfg(feature = "proto-gopher")]
    Gopher(GopherResponse),
    // #[cfg(feature = "proto-file")]
    // File(FileResponse),
    // #[cfg(feature = "proto-irc")]
    // Irc(IrcResponse),
}

pub struct Fetcher {
    #[cfg(feature = "proto-http")]
    http_fetcher: CompleteHttpFetcher, // This is the fetcher with the compiled request agent (ie: HttpAgent<ReqwestAgent>)
    #[cfg(feature = "proto-ftp")]
    ftp_fetcher: FtpFetcher,
    #[cfg(feature = "proto-gopher")]
    gopher_fetcher: GopherFetcher,
}

impl Fetcher {
    pub const fn protocols_implemented() -> &'static [&'static str] {
        &[
            #[cfg(feature = "proto-http")]
            "http",
            #[cfg(feature = "proto-ftp")]
            "ftp",
            // #[cfg(feature = "proto-file")]
            // "file",
            // #[cfg(feature = "proto-irc")]
            // "irc",
            #[cfg(feature = "proto-gopher")]
            "gopher",
        ]
    }

    pub fn new(base_url: Url) -> Self {
        Fetcher {
            #[cfg(feature = "proto-http")]
            http_fetcher: CompleteHttpFetcher::new(base_url.clone()),
            #[cfg(feature = "proto-ftp")]
            ftp_fetcher: FtpFetcher::new(base_url.clone()),
            #[cfg(feature = "proto-gopher")]
            gopher_fetcher: GopherFetcher::new(base_url.clone()),
        }
    }

    async fn fetch(&self, url: Url) -> Result<Response, FetcherError> {
        let scheme = url.scheme();

        match scheme {
            #[cfg(feature = "proto-http")]
            "https" | "http" => {
                let request = HttpRequestBuilder::new(HttpMethod::Get, url).build();
                match self.http_fetcher.fetch_with_request(request).await {
                    Ok(response) => Ok(Response::Http(response)),
                    Err(e) => Err(FetcherError::Http(e)),
                }
            }
            #[cfg(feature = "proto-ftp")]
            "ftp" => {
                let request = FtpRequest::new(url);
                match self.ftp_fetcher.fetch(request).await {
                    Ok(response) => Ok(Response::Ftp(response)),
                    Err(e) => Err(FetcherError::Ftp(e)),
                }
            }
            // "file" => {
            //     self.file_fetcher.fetch(url)
            // }
            // "irc" => {
            //     self.irc_fetcher.fetch(url)
            // }
            #[cfg(feature = "proto-gopher")]
            "gopher" => {
                let request = GopherRequest::new(url);
                match self.gopher_fetcher.fetch(request).await {
                    Ok(response) => Ok(Response::Gopher(response)),
                    Err(e) => Err(FetcherError::Gopher(e)),
                }
            }
            _ => {
                error!("Unsupported scheme: {}", scheme);
                Err(FetcherError::UnsupportedScheme(scheme.to_string()))
            }
        }
    }
}

pub async fn fetch_url_body(url: Url) -> Result<Vec<u8>, FetcherError> {
    let fetcher = Fetcher::new(url.clone());
    match fetcher.fetch(url).await {
        // There was a correct response
        Ok(response) => {
            match response {
                #[cfg(feature = "proto-gopher")]
                Response::Gopher(gopher_response) => {
                    let mut body = Vec::new();
                    for entry in gopher_response.entries {
                        body.extend_from_slice(entry.display_string.as_bytes());
                        body.extend_from_slice(b"\r\n");
                    }
                    Ok(body)
                }
                // It is an HTTP response
                #[cfg(feature = "proto-http")]
                Response::Http(http_response) => {
                    // It is a 200 OK response
                    if http_response.head().status_code() == 200 {
                        match http_response.body() {
                            // We've got a body
                            HttpBody::Reader(reader) => {
                                match reader.vec().await {
                                    // We've got the body as a Vec<u8>
                                    Ok(data) => Ok(data),
                                    Err(e) => {
                                        error!("Failed to fetch body from URL: {:?}", e);
                                        Err(FetcherError::Http(http::HttpError::UnknownError))
                                    }
                                }
                            }
                            HttpBody::Empty => Ok(Vec::new()),
                        }
                    } else {
                        Err(FetcherError::Http(http::HttpError::UnknownError))
                    }
                }
                #[allow(unreachable_patterns)]
                _ => {
                    error!("Unsupported response type. We expected a HTTP response");
                    Err(FetcherError::Http(http::HttpError::UnknownError))
                }
            }
        }
        Err(e) => Err(e),
    }
}
