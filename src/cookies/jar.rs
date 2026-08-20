use cookie::Cookie;
use reqwest::cookie::CookieStore;
use reqwest::header::HeaderValue;
use std::sync::{Arc, Mutex};
use url::Url;

pub trait StorageBackend: Send + Sync {
    fn store(&self, url: &Url, value: &Cookie);
    fn get(&self, url: &Url) -> Option<Vec<Cookie<'_>>>;
}

pub struct CookieJar {
    store: Arc<Mutex<dyn StorageBackend>>,
}

impl CookieJar {
    pub fn new(store: Arc<Mutex<dyn StorageBackend>>) -> Self {
        Self { store }
    }
}

impl CookieStore for CookieJar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &url::Url) {
        for header_value in cookie_headers {
            if let Ok(c) = Cookie::parse(header_value.to_str().unwrap()) {
                self.store.lock().unwrap().store(url, &c);
            }
        }
    }

    fn cookies(&self, url: &url::Url) -> Option<HeaderValue> {
        let locked_store = self.store.lock().unwrap();
        let cookies = locked_store.get(url);

        cookies.as_ref()?;

        let cookies = cookies.unwrap();
        let mut cookie_str = String::new();
        for cookie in cookies {
            cookie_str.push_str(&cookie.to_string());
            cookie_str.push_str("; ");
        }

        cookie_str.pop();
        cookie_str.pop();

        drop(locked_store);
        Some(HeaderValue::from_str(&cookie_str).unwrap())
    }
}
