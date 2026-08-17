use crate::cookies::jar::StorageBackend;
use cookie::Cookie;
use log::warn;
use rusqlite::{params, Connection};
use std::sync::Mutex;
use url::Url;
use uuid::Uuid;

pub struct SqliteStorage {
    /// Connection. Should be guarded through a mutex, as it can be used multi-threaded
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    pub fn new(database_path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(database_path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS cookies (
                id UUID PRIMARY KEY,
                domain TEXT NOT NULL,
                path TEXT NOT NULL,
                name TEXT NOT NULL,
                cookie TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS idx_dpn ON cookies (domain, path, name)", [])?;

        Ok(Self { conn: Mutex::new(conn) })
    }
}

impl StorageBackend for SqliteStorage {
    fn store(&self, url: &Url, cookie: &Cookie) {
        if cookie.expires().is_none() {
            // No expires found
            return;
        }

        let expires = cookie.expires().unwrap();
        if expires.is_session() {
            // Session cookie, do not store
            return;
        }

        let expires_at = expires.datetime().unwrap().unix_timestamp();

        let domain = match cookie.domain() {
            Some(d) => d.to_string(),
            None => url.domain().unwrap().to_string(),
        };

        let e = self.conn.lock().unwrap().execute(
            "INSERT INTO cookies (id, domain, path, name, cookie, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(domain, path, name) DO UPDATE SET cookie = excluded.cookie, expires_at = excluded.expires_at",
            params![
                Uuid::now_v7().to_string(),
                domain,
                cookie.path(),
                cookie.name(),
                cookie.to_string(),
                expires_at
            ],
        );
        if let Err(e) = e {
            warn!("failed to store cookie: {:?}", e);
        }
    }

    fn get(&self, url: &Url) -> Option<Vec<Cookie<'_>>> {
        let domain = url.domain().unwrap().to_string();
        let path = url.path().to_string();

        let locked_conn = self.conn.lock().unwrap();

        let stmt = locked_conn.prepare("SELECT cookie FROM cookies WHERE domain = ?1 AND path = ?2 AND expires_at > ?3");
        if let Err(e) = stmt {
            warn!("failed to prepare statement: {:?}", e);
            return None;
        }

        let mut stmt = stmt.unwrap();
        let cookies = stmt
            .query_map(params![domain, path, chrono::Utc::now().timestamp()], |row| {
                let cookie_str: String = row.get(0)?;
                Ok(Cookie::parse(cookie_str).unwrap())
            })
            .unwrap()
            .map(|c| c.unwrap())
            .collect();

        Some(cookies)
    }
}
