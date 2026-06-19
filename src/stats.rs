use anyhow::{Context, Result};
use serde_json::Value;

/// A snapshot of a KurrentDB node's server-side statistics, distilled from the
/// HTTP `/stats` endpoint into just the fields the dashboard charts.
#[derive(Debug, Clone, Default)]
pub struct ServerStats {
    /// Process CPU usage, percent (0..=100, but can exceed 100 on multicore).
    pub proc_cpu: f64,
    /// Total system memory, bytes.
    pub sys_total_mem: u64,
    /// Free system memory, bytes.
    pub sys_free_mem: u64,
    /// Cumulative disk bytes read by the process.
    pub disk_read_bytes: u64,
    /// Cumulative disk bytes written by the process.
    pub disk_write_bytes: u64,
    /// Cumulative disk read operations by the process.
    pub disk_read_ops: u64,
    /// Cumulative disk write operations by the process.
    pub disk_write_ops: u64,
    /// Storage reader queue throughput, items/second.
    pub reader_items_per_sec: f64,
    /// Storage writer queue throughput, items/second.
    pub writer_items_per_sec: f64,
    /// Cumulative read-index record cache hits.
    pub cached_record: u64,
    /// Cumulative read-index record cache misses.
    pub not_cached_record: u64,
    /// Per-queue `(name, length, avg_items_per_second)`.
    pub queues: Vec<(String, u64, f64)>,
}

impl ServerStats {
    pub fn mem_used(&self) -> u64 {
        self.sys_total_mem.saturating_sub(self.sys_free_mem)
    }

    fn from_json(v: &Value) -> Self {
        let mut stats = ServerStats {
            proc_cpu: get_f64(v, "proc-cpu").unwrap_or(0.0),
            sys_total_mem: get_u64(v, "sys-totalMem").unwrap_or(0),
            sys_free_mem: get_u64(v, "sys-freeMem").unwrap_or(0),
            disk_read_bytes: get_u64(v, "proc-diskIo-readBytes").unwrap_or(0),
            disk_write_bytes: get_u64(v, "proc-diskIo-writtenBytes").unwrap_or(0),
            disk_read_ops: get_u64(v, "proc-diskIo-readOps").unwrap_or(0),
            disk_write_ops: get_u64(v, "proc-diskIo-writeOps").unwrap_or(0),
            reader_items_per_sec: get_f64(v, "es-queue-storageReaderQueue-avgItemsPerSecond")
                .unwrap_or(0.0),
            writer_items_per_sec: get_f64(v, "es-queue-storageWriterQueue-avgItemsPerSecond")
                .unwrap_or(0.0),
            cached_record: get_u64(v, "es-readIndex-cachedRecord").unwrap_or(0),
            not_cached_record: get_u64(v, "es-readIndex-notCachedRecord").unwrap_or(0),
            queues: Vec::new(),
        };

        // Queues live under es.queue (nested) as an object of name -> { length, avgItemsPerSecond }.
        if let Some(Value::Object(queues)) = nested(v, "es-queue") {
            for (name, q) in queues {
                let len = q.get("length").and_then(Value::as_u64).unwrap_or(0);
                let rate = q
                    .get("avgItemsPerSecond")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                stats.queues.push((name.clone(), len, rate));
            }
            // Stable, descending by length so the dashboard shows hot queues first.
            stats.queues.sort_by(|a, b| b.1.cmp(&a.1));
        }

        stats
    }
}

/// Look up a value that may be stored either as a flat dotted/dashed key
/// (`"proc-cpu"`) or nested (`{"proc": {"cpu": ...}}`).
fn lookup<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(found) = v.get(key) {
        return Some(found);
    }
    nested(v, key)
}

/// Traverse a nested object by splitting `key` on '-'.
fn nested<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in key.split('-') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn get_f64(v: &Value, key: &str) -> Option<f64> {
    lookup(v, key).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}

fn get_u64(v: &Value, key: &str) -> Option<u64> {
    lookup(v, key).and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_f64().map(|f| f as u64))
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

/// Polls a KurrentDB node's `/stats` endpoint over HTTP.
#[derive(Clone)]
pub struct StatsClient {
    http: reqwest::Client,
    url: String,
    username: String,
    password: String,
}

impl StatsClient {
    pub fn new(url: String, username: String, password: String) -> Self {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_default();
        StatsClient {
            http,
            url,
            username,
            password,
        }
    }

    pub async fn poll(&self) -> Result<ServerStats> {
        let resp = self
            .http
            .get(&self.url)
            .basic_auth(&self.username, Some(&self.password))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .context("requesting /stats")?
            .error_for_status()
            .context("/stats returned an error status")?;
        let value: Value = resp.json().await.context("decoding /stats JSON")?;
        Ok(ServerStats::from_json(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the nested shape KurrentDB's /stats actually returns.
    #[test]
    fn parses_nested_stats() {
        let json = serde_json::json!({
            "proc": {
                "cpu": 12.5,
                "diskIo": {
                    "readBytes": 1000u64, "writtenBytes": 2000u64,
                    "readOps": 30u64, "writeOps": 40u64
                }
            },
            "sys": { "totalMem": 9_000_000u64, "freeMem": 3_000_000u64 },
            "es": {
                "queue": {
                    "index Committer": { "length": 5u64, "avgItemsPerSecond": 1.5 },
                    "Writer": { "length": 12u64, "avgItemsPerSecond": 8.0 },
                    "storageReaderQueue": { "length": 0u64, "avgItemsPerSecond": 3.5 },
                    "storageWriterQueue": { "length": 0u64, "avgItemsPerSecond": 6.0 }
                },
                "readIndex": { "cachedRecord": 7000u64, "notCachedRecord": 11u64 }
            }
        });

        let stats = ServerStats::from_json(&json);
        assert_eq!(stats.proc_cpu, 12.5);
        assert_eq!(stats.disk_read_bytes, 1000);
        assert_eq!(stats.disk_write_bytes, 2000);
        assert_eq!(stats.disk_read_ops, 30);
        assert_eq!(stats.disk_write_ops, 40);
        assert_eq!(stats.reader_items_per_sec, 3.5);
        assert_eq!(stats.writer_items_per_sec, 6.0);
        assert_eq!(stats.cached_record, 7000);
        assert_eq!(stats.not_cached_record, 11);
        assert_eq!(stats.sys_total_mem, 9_000_000);
        assert_eq!(stats.mem_used(), 6_000_000);
        // Sorted descending by length: Writer (12) before the others.
        assert_eq!(stats.queues.len(), 4);
        assert_eq!(stats.queues[0].0, "Writer");
        assert_eq!(stats.queues[0].1, 12);
    }

    /// Also accepts the flat dotted-key format used by older versions / $stats events.
    #[test]
    fn parses_flat_stats() {
        let json = serde_json::json!({
            "proc-cpu": 7.0,
            "sys-totalMem": 100u64,
            "sys-freeMem": 40u64,
        });
        let stats = ServerStats::from_json(&json);
        assert_eq!(stats.proc_cpu, 7.0);
        assert_eq!(stats.mem_used(), 60);
    }
}
