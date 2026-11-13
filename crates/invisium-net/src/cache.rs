//! Memory-only HTTP cache, partitioned by top-level site, bounded by a byte budget (LRU).
//! Only explicitly fresh responses (`max-age`) are stored; `no-store` / `no-cache` never are.

use crate::types::Headers;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub(crate) struct Cached {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
    /// `Response::served_local` of the response this entry was stored from.
    pub served_local: bool,
}

struct Entry {
    resp: Cached,
    expires: Instant,
    size: usize,
    used: u64,
}

#[derive(Default)]
pub(crate) struct Cache {
    map: HashMap<(String, String), Entry>,
    bytes: usize,
    tick: u64,
}

/// Freshness lifetime if the response may be stored, else `None`.
pub(crate) fn freshness(status: u16, headers: &Headers) -> Option<Duration> {
    if !matches!(status, 200 | 203 | 204 | 404 | 410) || headers.get("set-cookie").is_some() {
        return None;
    }
    let vary_ok = headers
        .get_all("vary")
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .all(|v| v.is_empty() || v.eq_ignore_ascii_case("accept-encoding"));
    if !vary_ok {
        return None;
    }
    let mut max_age = None;
    for d in headers.get_all("cache-control").flat_map(|v| v.split(',')) {
        let (k, v) = d.split_once('=').unwrap_or((d, ""));
        match k.trim().to_ascii_lowercase().as_str() {
            "no-store" | "no-cache" => return None,
            "max-age" => max_age = v.trim().trim_matches('"').parse::<u64>().ok(),
            _ => {}
        }
    }
    max_age.filter(|&s| s > 0).map(Duration::from_secs)
}

impl Cache {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    pub fn get(&mut self, partition: &str, url: &str, now: Instant) -> Option<Cached> {
        let key = (partition.to_string(), url.to_string());
        let fresh = self.map.get(&key).map(|e| e.expires > now)?;
        if !fresh {
            self.remove(&key);
            return None;
        }
        self.tick += 1;
        let e = self.map.get_mut(&key)?;
        e.used = self.tick;
        Some(e.resp.clone())
    }

    fn remove(&mut self, key: &(String, String)) {
        if let Some(e) = self.map.remove(key) {
            self.bytes -= e.size;
        }
    }

    /// Evicts least-recently-used entries until at most `budget` bytes remain.
    pub fn trim(&mut self, budget: usize) {
        while self.bytes > budget {
            let Some(k) = self.map.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else {
                break;
            };
            self.remove(&k);
        }
    }

    pub fn put(&mut self, partition: &str, url: &str, resp: Cached, ttl: Duration, budget: usize, now: Instant) {
        let size = resp.body.len()
            + url.len()
            + resp.headers.iter().map(|(n, v)| n.len() + v.len()).sum::<usize>()
            + 64;
        let key = (partition.to_string(), url.to_string());
        self.remove(&key);
        if size > budget {
            return;
        }
        self.trim(budget - size);
        self.tick += 1;
        self.bytes += size;
        self.map.insert(key, Entry { resp, expires: now + ttl, size, used: self.tick });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(cc: &str, body: usize) -> (Cached, Option<Duration>) {
        let mut h = Headers::new();
        if !cc.is_empty() {
            h.push("Cache-Control", cc);
        }
        let ttl = freshness(200, &h);
        (Cached { status: 200, headers: h, body: vec![b'x'; body], served_local: false }, ttl)
    }

    #[test]
    fn max_age_hit_and_expiry() {
        let mut c = Cache::default();
        let now = Instant::now();
        let (r, ttl) = resp("public, max-age=60", 10);
        c.put("a.com", "https://a.com/x", r, ttl.unwrap(), 1 << 20, now);
        assert_eq!(c.get("a.com", "https://a.com/x", now).unwrap().body.len(), 10);
        assert!(c.get("a.com", "https://a.com/x", now + Duration::from_secs(61)).is_none());
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn no_store_and_uncacheable_are_not_stored() {
        assert_eq!(resp("no-store, max-age=60", 1).1, None);
        assert_eq!(resp("no-cache", 1).1, None);
        assert_eq!(resp("", 1).1, None);
        assert_eq!(resp("max-age=0", 1).1, None);
        let mut h = Headers::new();
        h.push("Cache-Control", "max-age=60");
        h.push("Set-Cookie", "a=b");
        assert_eq!(freshness(200, &h), None);
        assert_eq!(freshness(500, &Headers::new()), None);
    }

    #[test]
    fn partitioned_by_top_level_site() {
        let mut c = Cache::default();
        let now = Instant::now();
        let (r, ttl) = resp("max-age=60", 5);
        c.put("site-a.com", "https://cdn.com/lib.js", r, ttl.unwrap(), 1 << 20, now);
        assert!(c.get("site-a.com", "https://cdn.com/lib.js", now).is_some());
        assert!(c.get("site-b.com", "https://cdn.com/lib.js", now).is_none());
    }

    #[test]
    fn evicts_lru_under_byte_budget() {
        let mut c = Cache::default();
        let now = Instant::now();
        let budget = 600;
        for u in ["https://a/1", "https://a/2"] {
            let (r, ttl) = resp("max-age=60", 200);
            c.put("a", u, r, ttl.unwrap(), budget, now);
        }
        c.get("a", "https://a/1", now); // 1 is now more recently used than 2
        let (r, ttl) = resp("max-age=60", 200);
        c.put("a", "https://a/3", r, ttl.unwrap(), budget, now);
        assert!(c.bytes() <= budget);
        assert!(c.get("a", "https://a/2", now).is_none());
        assert!(c.get("a", "https://a/1", now).is_some() && c.get("a", "https://a/3", now).is_some());
        let (big, ttl) = resp("max-age=60", 10_000);
        c.put("a", "https://a/big", big, ttl.unwrap(), budget, now);
        assert!(c.get("a", "https://a/big", now).is_none());
    }
}
