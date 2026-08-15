use crate::cookies::jar::CookieJar;
use crate::cookies::sqlite_store::SqliteStorage;
use crate::fetcher::async_stream::AsyncStream;
use crate::fetcher::http::agents::HttpRequestAgent;
use crate::fetcher::http::request::HttpRequest;
use crate::fetcher::http::response::{HttpResponse, HttpVersion, ResponseHeader};
use crate::fetcher::http::HttpError;
use crate::fetcher::http::{HttpBody, GOSUB_USERAGENT_STRING};
use crate::fetcher::HttpMethod;
use log::info;
use reqwest::header::{HeaderMap, HeaderName};
use reqwest::{Method, Version};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Http agent that uses reqwest library to make HTTP requests
pub struct ReqwestAgent {
    client: reqwest::Client,
}

impl HttpRequestAgent for ReqwestAgent {
    fn new() -> Self {
        // Create the actual client that will handle the requests
        let mut builder = reqwest::Client::builder()
            .user_agent(GOSUB_USERAGENT_STRING)
            .timeout(Duration::from_secs(5))
            .use_rustls_tls() // For HTTP2
            .connect_timeout(Duration::from_secs(5))
            .connection_verbose(true)
            .read_timeout(Duration::from_secs(5))
            .brotli(true)
            .gzip(true)
            .deflate(true);

        // Cookies should be dealt with more flexible
        let jar_path = crate::engine::data_dir().join("fetcher-cookies.db");
        let jar = match SqliteStorage::new(&jar_path.to_string_lossy()) {
            Ok(store) => {
                info!(target: "fetcher", "successfully created SqliteStorage");
                Some(CookieJar::new(Arc::new(Mutex::new(store))))
            }
            Err(e) => {
                info!(target: "fetcher", "failed to create SqliteStorage: {:?}", e);
                None
            }
        };

        match jar {
            Some(jar) => {
                builder = builder.cookie_provider(Arc::new(jar));
            }
            None => {
                info!(target: "fetcher", "no cookie jar");
            }
        }

        Self {
            client: builder.build().unwrap(),
        }
    }

    async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        println!("executing request via Reqwest: {:?}", req);

        // Additional headers we like to add, besides the ones given in the request
        let mut headers = HeaderMap::new();
        // headers.insert("User-Agent", GOSUB_USERAGENT_STRING.parse().unwrap());
        // headers.insert("Connection", "keep-alive".parse().unwrap());
        headers.insert(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".parse().unwrap(),
        );
        headers.insert("Accept-Language", "en-US,en;q=0.5".parse().unwrap());
        headers.insert("DNT", "1".parse().unwrap());

        for (key, value) in req.headers {
            headers.insert(HeaderName::from_str(&key).unwrap(), value.parse().unwrap());
        }

        // Cookies are automatically added through the CookieJar we defined in the constructor

        let request_builder = self.client.request(req.method.into(), req.url.clone()).headers(headers.clone());
        let request = request_builder.build().map_err(|e| HttpError::AgentError(Box::new(e)))?;

        match self.client.execute(request).await {
            Ok(response) => {
                let res_header = ResponseHeader::new(
                    response.version().into(),
                    response.status().into(),
                    response
                        .headers()
                        .iter()
                        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap().to_string()))
                        .collect(),
                    response.content_length(),
                    response.cookies().map(|c| (c.name().to_string(), c.value().to_string())).collect(),
                    req.url.clone(),
                );

                // Check if we have a content length, if so, we can decide if we want to read the
                // body as a whole (if small enough), or we need to stream it
                let body = match response.content_length() {
                    //TODO: it might not be correct to decide on the content length what to do
                    Some(len) => {
                        if len == 0 {
                            // Length is explicitly 0, we assume there isn't a body found
                            HttpBody::Empty
                        } else {
                            HttpBody::Reader(AsyncStream::new(response.bytes_stream(), Some(len as usize)))
                        }
                    }
                    None => {
                        // No information is present about the body, we return a streaming body as default
                        info!(target: "fetcher", "no content length given. Assume streaming");
                        HttpBody::Reader(AsyncStream::new(response.bytes_stream(), None))
                    }
                };

                info!(target: "fetcher", "response: {:?}", res_header);
                Ok(HttpResponse::new(res_header, body))
            }
            Err(e) => {
                info!(target: "fetcher", "error: {:?}", e);

                Err(HttpError::AgentError(Box::new(e))) //TODO: we need to map the actual errors to the `HttpError`
            }
        }
    }
}

impl From<reqwest::Version> for HttpVersion {
    fn from(value: Version) -> Self {
        match value {
            Version::HTTP_09 => HttpVersion::Http09,
            Version::HTTP_10 => HttpVersion::Http10,
            Version::HTTP_11 => HttpVersion::Http11,
            Version::HTTP_2 => HttpVersion::Http2,
            Version::HTTP_3 => HttpVersion::Http3,
            _ => HttpVersion::Http11,
        }
    }
}

impl From<reqwest::Method> for HttpMethod {
    fn from(value: Method) -> Self {
        match value {
            Method::OPTIONS => HttpMethod::Options,
            Method::GET => HttpMethod::Get,
            Method::PUT => HttpMethod::Put,
            Method::POST => HttpMethod::Post,
            Method::DELETE => HttpMethod::Delete,
            Method::TRACE => HttpMethod::Trace,
            Method::PATCH => HttpMethod::Patch,
            Method::HEAD => HttpMethod::Head,
            Method::CONNECT => HttpMethod::Connect,
            _ => HttpMethod::Custom(value.to_string()),
        }
    }
}
impl From<HttpMethod> for Method {
    fn from(value: HttpMethod) -> Self {
        match value {
            HttpMethod::Options => Method::OPTIONS,
            HttpMethod::Get => Method::GET,
            HttpMethod::Put => Method::PUT,
            HttpMethod::Post => Method::POST,
            HttpMethod::Delete => Method::DELETE,
            HttpMethod::Trace => Method::TRACE,
            HttpMethod::Patch => Method::PATCH,
            HttpMethod::Head => Method::HEAD,
            HttpMethod::Connect => Method::CONNECT,
            HttpMethod::Custom(s) => Method::from_bytes(s.as_bytes()).unwrap(),
        }
    }
}
