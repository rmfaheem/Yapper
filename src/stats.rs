use anyhow::{Context, Result};
use serde_json::Value;

/// One KurrentDB processing queue, as reported under `/stats` `es-queue`.
#[derive(Debug, Clone, Default)]
pub struct QueueStat {
    /// Queue name (e.g. `storageReaderQueue`, `persistentSubscriptions`).
    pub name: String,
    /// Peak length reached during the current measurement cycle — the most
    /// useful backlog signal, since the instantaneous `length` is sampled too
    /// coarsely to catch transient pile-ups.
    pub length_current_try_peak: u64,
}

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
    /// Storage reader queue peak backlog this cycle (`lengthCurrentTryPeak`).
    pub reader_queue_peak: u64,
    /// Storage writer queue peak backlog this cycle (`lengthCurrentTryPeak`).
    pub writer_queue_peak: u64,
    /// Cumulative read-index record cache hits.
    pub cached_record: u64,
    /// Cumulative read-index record cache misses.
    pub not_cached_record: u64,
    /// Current number of TCP connections.
    pub tcp_connections: u64,
    /// TCP receive throughput, bytes/second.
    pub tcp_receiving_speed: f64,
    /// TCP send throughput, bytes/second.
    pub tcp_sending_speed: f64,
    /// Every processing queue reported under `es-queue`, hottest peak first.
    pub queues: Vec<QueueStat>,
}

impl ServerStats {
    /// Queues whose name looks like a persistent-subscription queue.
    pub fn persistent_sub_queues(&self) -> impl Iterator<Item = &QueueStat> {
        self.queues.iter().filter(|q| {
            let n = q.name.to_lowercase();
            n.contains("persistent") || n.contains("subscription")
        })
    }
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
            reader_queue_peak: get_u64(v, "es-queue-storageReaderQueue-lengthCurrentTryPeak")
                .unwrap_or(0),
            writer_queue_peak: get_u64(v, "es-queue-storageWriterQueue-lengthCurrentTryPeak")
                .unwrap_or(0),
            cached_record: get_u64(v, "es-readIndex-cachedRecord").unwrap_or(0),
            not_cached_record: get_u64(v, "es-readIndex-notCachedRecord").unwrap_or(0),
            tcp_connections: get_u64(v, "proc-tcp-connections").unwrap_or(0),
            tcp_receiving_speed: get_f64(v, "proc-tcp-receivingSpeed").unwrap_or(0.0),
            tcp_sending_speed: get_f64(v, "proc-tcp-sendingSpeed").unwrap_or(0.0),
            queues: Vec::new(),
        };

        // Queues live under es.queue (nested) as an object of
        // name -> { length, lengthCurrentTryPeak, avgItemsPerSecond }.
        if let Some(Value::Object(queues)) = nested(v, "es-queue") {
            for (name, q) in queues {
                stats.queues.push(QueueStat {
                    name: name.clone(),
                    length_current_try_peak: q
                        .get("lengthCurrentTryPeak")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                });
            }
            // Stable, descending by peak backlog so the dashboard shows the
            // queues under the most pressure first.
            stats
                .queues
                .sort_by(|a, b| b.length_current_try_peak.cmp(&a.length_current_try_peak));
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
                },
                "tcp": { "connections": 17u64, "receivingSpeed": 1234.0, "sendingSpeed": 5678.0 }
            },
            "sys": { "totalMem": 9_000_000u64, "freeMem": 3_000_000u64 },
            "es": {
                "queue": {
                    "index Committer": { "length": 5u64, "lengthCurrentTryPeak": 9u64, "avgItemsPerSecond": 1.5 },
                    "Writer": { "length": 12u64, "lengthCurrentTryPeak": 40u64, "avgItemsPerSecond": 8.0 },
                    "storageReaderQueue": { "length": 0u64, "lengthCurrentTryPeak": 7u64, "avgItemsPerSecond": 3.5 },
                    "storageWriterQueue": { "length": 0u64, "lengthCurrentTryPeak": 13u64, "avgItemsPerSecond": 6.0 },
                    "persistentSubscriptions": { "length": 2u64, "lengthCurrentTryPeak": 55u64, "avgItemsPerSecond": 4.0 }
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
        assert_eq!(stats.reader_queue_peak, 7);
        assert_eq!(stats.writer_queue_peak, 13);
        assert_eq!(stats.cached_record, 7000);
        assert_eq!(stats.not_cached_record, 11);
        assert_eq!(stats.sys_total_mem, 9_000_000);
        assert_eq!(stats.mem_used(), 6_000_000);
        assert_eq!(stats.tcp_connections, 17);
        assert_eq!(stats.tcp_receiving_speed, 1234.0);
        assert_eq!(stats.tcp_sending_speed, 5678.0);
        // Sorted descending by peak backlog: persistentSubscriptions (55) first.
        assert_eq!(stats.queues.len(), 5);
        assert_eq!(stats.queues[0].name, "persistentSubscriptions");
        assert_eq!(stats.queues[0].length_current_try_peak, 55);
        // The persistent-subscription queue is detected by name.
        let psubs: Vec<&str> = stats.persistent_sub_queues().map(|q| q.name.as_str()).collect();
        assert_eq!(psubs, ["persistentSubscriptions"]);
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
