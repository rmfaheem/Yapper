use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::db::FloodParams;
use crate::metrics::Metrics;
use crate::stats::ServerStats;

/// How many samples of history to keep for the sparklines.
pub const HISTORY: usize = 120;

/// The server-stats dashboard tabs, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashTab {
    System,
    Disk,
    Queues,
    Cache,
}

impl DashTab {
    /// All tabs, in display order.
    pub const ALL: [DashTab; 4] = [
        DashTab::System,
        DashTab::Disk,
        DashTab::Queues,
        DashTab::Cache,
    ];

    pub fn title(self) -> &'static str {
        match self {
            DashTab::System => "System",
            DashTab::Disk => "Disk",
            DashTab::Queues => "Queues",
            DashTab::Cache => "Cache",
        }
    }
}

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Stats(ServerStats),
    StatsError(String),
    FloodFinished(String),
}

/// What the event loop should do after handling a key.
pub enum Outcome {
    None,
    Quit,
    StartFlood { write: bool, params: FloodParams, label: String },
}

pub struct App {
    /// The command line buffer.
    pub input: String,
    /// Cursor position within `input`, as a char index.
    pub cursor: usize,
    /// The scrolling console log (command echoes, results, errors).
    pub output: Vec<String>,
    pub show_help: bool,
    pub metrics: Arc<Metrics>,
    pub flood_running: bool,
    pub current_flood: String,

    // Command history (most recent last). `history_idx` is the position we're
    // browsing while pressing Up/Down; `None` means we're on the live buffer.
    history: Vec<String>,
    history_idx: Option<usize>,

    // Client-side history / derived values.
    pub throughput: VecDeque<u64>,
    pub p50: f64,
    pub p99: f64,
    last_ops: u64,
    last_tick: Instant,

    // Which server-stats tab is showing.
    pub active_tab: usize,

    // Server-side stats + history.
    pub stats: Option<ServerStats>,
    pub cpu_hist: VecDeque<u64>,
    pub disk_read_hist: VecDeque<u64>,
    pub disk_write_hist: VecDeque<u64>,
    pub disk_read_ops_hist: VecDeque<u64>,
    pub disk_write_ops_hist: VecDeque<u64>,
    pub reader_ips_hist: VecDeque<u64>,
    pub writer_ips_hist: VecDeque<u64>,
    pub cache_hit_hist: VecDeque<u64>,
    pub cache_miss_hist: VecDeque<u64>,
    last_disk_read: Option<u64>,
    last_disk_write: Option<u64>,
    last_disk_read_ops: Option<u64>,
    last_disk_write_ops: Option<u64>,
    last_cached: Option<u64>,
    last_not_cached: Option<u64>,
    last_stats_at: Option<Instant>,
}

impl App {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        App {
            input: String::new(),
            cursor: 0,
            output: vec![
                "Welcome to Yapper 🗣  — type `help` for commands.".to_string(),
            ],
            show_help: false,
            metrics,
            flood_running: false,
            current_flood: String::new(),
            history: Vec::new(),
            history_idx: None,
            throughput: VecDeque::new(),
            p50: 0.0,
            p99: 0.0,
            last_ops: 0,
            last_tick: Instant::now(),
            active_tab: 0,
            stats: None,
            cpu_hist: VecDeque::new(),
            disk_read_hist: VecDeque::new(),
            disk_write_hist: VecDeque::new(),
            disk_read_ops_hist: VecDeque::new(),
            disk_write_ops_hist: VecDeque::new(),
            reader_ips_hist: VecDeque::new(),
            writer_ips_hist: VecDeque::new(),
            cache_hit_hist: VecDeque::new(),
            cache_miss_hist: VecDeque::new(),
            last_disk_read: None,
            last_disk_write: None,
            last_disk_read_ops: None,
            last_disk_write_ops: None,
            last_cached: None,
            last_not_cached: None,
            last_stats_at: None,
        }
    }

    /// Cycle the dashboard tab forward (Tab) or backward (Shift-Tab).
    pub fn next_tab(&mut self) {
        self.active_tab = (self.active_tab + 1) % DashTab::ALL.len();
    }

    pub fn prev_tab(&mut self) {
        let n = DashTab::ALL.len();
        self.active_tab = (self.active_tab + n - 1) % n;
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        self.output.push(line.into());
        // Bound the console history.
        if self.output.len() > 1000 {
            let overflow = self.output.len() - 1000;
            self.output.drain(0..overflow);
        }
    }

    /// Compute client-side per-tick throughput and latency percentiles.
    pub fn on_tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().max(0.001);
        let ops = self.metrics.total_ops();
        let delta = ops.saturating_sub(self.last_ops);
        let per_sec = (delta as f64 / dt).round() as u64;
        push_capped(&mut self.throughput, per_sec);
        self.last_ops = ops;
        self.last_tick = now;

        let (p50, p99) = self.metrics.drain_latency_percentiles();
        // Keep last non-zero percentiles when no samples arrived this tick.
        if p50 > 0.0 || p99 > 0.0 {
            self.p50 = p50;
            self.p99 = p99;
        }
    }

    pub fn on_stats(&mut self, stats: ServerStats) {
        let now = Instant::now();
        push_capped(&mut self.cpu_hist, stats.proc_cpu.clamp(0.0, 100.0) as u64);

        // Queue throughput is already a rate reported by the server, so it can
        // be charted directly without a previous sample.
        push_capped(&mut self.reader_ips_hist, stats.reader_items_per_sec.max(0.0) as u64);
        push_capped(&mut self.writer_ips_hist, stats.writer_items_per_sec.max(0.0) as u64);

        // The remaining series are cumulative counters; convert each to a
        // per-second rate over the interval since the previous poll.
        if let Some(prev_t) = self.last_stats_at {
            let dt = now.duration_since(prev_t).as_secs_f64().max(0.001);
            let rate = |cur: u64, prev: Option<u64>| -> u64 {
                prev.map(|p| (cur.saturating_sub(p) as f64 / dt) as u64)
                    .unwrap_or(0)
            };
            push_capped(&mut self.disk_read_hist, rate(stats.disk_read_bytes, self.last_disk_read));
            push_capped(&mut self.disk_write_hist, rate(stats.disk_write_bytes, self.last_disk_write));
            push_capped(&mut self.disk_read_ops_hist, rate(stats.disk_read_ops, self.last_disk_read_ops));
            push_capped(&mut self.disk_write_ops_hist, rate(stats.disk_write_ops, self.last_disk_write_ops));
            push_capped(&mut self.cache_hit_hist, rate(stats.cached_record, self.last_cached));
            push_capped(&mut self.cache_miss_hist, rate(stats.not_cached_record, self.last_not_cached));
        }
        self.last_disk_read = Some(stats.disk_read_bytes);
        self.last_disk_write = Some(stats.disk_write_bytes);
        self.last_disk_read_ops = Some(stats.disk_read_ops);
        self.last_disk_write_ops = Some(stats.disk_write_ops);
        self.last_cached = Some(stats.cached_record);
        self.last_not_cached = Some(stats.not_cached_record);
        self.last_stats_at = Some(now);
        self.stats = Some(stats);
    }

    /// Handle a key press, returning what the event loop should do.
    ///
    /// The command line is always focused — there are no modes to switch
    /// between. The dashboard is always visible behind the input.
    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Ctrl-C / Ctrl-D quit from anywhere.
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d')) {
            return Outcome::Quit;
        }

        match key.code {
            KeyCode::Esc => {
                // Only ever dismisses the help overlay; never quits, so it's
                // safe to mash while typing.
                self.show_help = false;
            }
            KeyCode::Enter => return self.submit(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.input.chars().count());
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            // Tab / Shift-Tab flip the server-stats dashboard tabs. They never
            // reach the input buffer, so typing is unaffected.
            KeyCode::Tab => self.next_tab(),
            KeyCode::BackTab => self.prev_tab(),
            KeyCode::Char(c) => self.insert(c),
            _ => {}
        }
        Outcome::None
    }

    // --- line editing ---------------------------------------------------

    /// Byte offset within `input` for the current char cursor.
    fn byte_offset(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_offset();
        self.input.insert(at, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let at = self.byte_offset();
        self.input.remove(at);
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        let at = self.byte_offset();
        self.input.remove(at);
    }

    fn set_input(&mut self, value: String) {
        self.cursor = value.chars().count();
        self.input = value;
    }

    // --- command history ------------------------------------------------

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            Some(0) => 0,
            Some(i) => i - 1,
            None => self.history.len() - 1,
        };
        self.history_idx = Some(idx);
        self.set_input(self.history[idx].clone());
    }

    fn history_next(&mut self) {
        match self.history_idx {
            Some(i) if i + 1 < self.history.len() => {
                self.history_idx = Some(i + 1);
                self.set_input(self.history[i + 1].clone());
            }
            Some(_) => {
                // Stepped past the newest entry: back to a fresh line.
                self.history_idx = None;
                self.set_input(String::new());
            }
            None => {}
        }
    }

    fn submit(&mut self) -> Outcome {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        self.history_idx = None;
        if line.is_empty() {
            return Outcome::None;
        }
        // Record in history (skip consecutive duplicates).
        if self.history.last().map(String::as_str) != Some(line.as_str()) {
            self.history.push(line.clone());
        }

        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        match cmd {
            "help" => {
                self.show_help = true;
                Outcome::None
            }
            "clear" => {
                self.output.clear();
                Outcome::None
            }
            "quit" | "exit" => Outcome::Quit,
            "wrfl" | "rdfl" => {
                let write = cmd == "wrfl";
                let params = parse_flood(&line, write);
                self.push_log(format!("> {line}"));
                Outcome::StartFlood {
                    write,
                    params,
                    label: line,
                }
            }
            _ => {
                self.push_log(format!("Unknown command: {line}"));
                Outcome::None
            }
        }
    }
}

fn push_capped(buf: &mut VecDeque<u64>, value: u64) {
    if buf.len() >= HISTORY {
        buf.pop_front();
    }
    buf.push_back(value);
}

/// Parse a `wrfl`/`rdfl` command line into FloodParams. Unknown flags are ignored;
/// missing flags fall back to sensible defaults.
fn parse_flood(line: &str, write: bool) -> FloodParams {
    let mut params = FloodParams {
        clients: 1,
        requests: 1,
        streams: 1,
        event_size: 10,
        batch_size: if write { 1 } else { 100 },
        stream_prefix: String::new(),
    };

    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut i = 1; // skip the command word
    while i < tokens.len() {
        let flag = tokens[i];
        let value = tokens.get(i + 1).copied();
        let mut consumed_value = true;
        match flag {
            "-c" | "--clients" => set_usize(&mut params.clients, value),
            "-r" | "--requests" => set_usize(&mut params.requests, value),
            "-s" | "--streams" => set_usize(&mut params.streams, value),
            "-e" | "--event-size" => set_usize(&mut params.event_size, value),
            "-b" | "--batch-size" => set_usize(&mut params.batch_size, value),
            "-p" | "--stream-prefix" => {
                if let Some(v) = value {
                    params.stream_prefix = v.to_string();
                }
            }
            _ => consumed_value = false,
        }
        i += if consumed_value && value.is_some() { 2 } else { 1 };
    }
    params
}

fn set_usize(target: &mut usize, value: Option<&str>) {
    if let Some(v) = value {
        if let Ok(n) = v.parse::<usize>() {
            *target = n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typ(app: &mut App, s: &str) {
        for c in s.chars() {
            let _ = app.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn parse_flood_write_defaults() {
        let p = parse_flood("wrfl", true);
        assert_eq!(p.clients, 1);
        assert_eq!(p.requests, 1);
        assert_eq!(p.streams, 1);
        assert_eq!(p.event_size, 10);
        assert_eq!(p.batch_size, 1); // write default
        assert_eq!(p.stream_prefix, "");
    }

    #[test]
    fn parse_flood_read_default_batch_is_100() {
        let p = parse_flood("rdfl", false);
        assert_eq!(p.batch_size, 100);
    }

    #[test]
    fn parse_flood_all_short_flags() {
        let p = parse_flood("wrfl -c 4 -r 1000 -s 10 -e 50 -b 5 -p yap", true);
        assert_eq!(p.clients, 4);
        assert_eq!(p.requests, 1000);
        assert_eq!(p.streams, 10);
        assert_eq!(p.event_size, 50);
        assert_eq!(p.batch_size, 5);
        assert_eq!(p.stream_prefix, "yap");
    }

    #[test]
    fn parse_flood_long_flags_and_unknown_are_ignored() {
        let p = parse_flood("rdfl --clients 8 --requests 3 --bogus zzz -p pre", false);
        assert_eq!(p.clients, 8);
        assert_eq!(p.requests, 3);
        assert_eq!(p.stream_prefix, "pre");
    }

    #[test]
    fn parse_flood_invalid_number_keeps_default() {
        let p = parse_flood("wrfl -c notanumber", true);
        assert_eq!(p.clients, 1);
    }

    #[test]
    fn submit_clear_empties_output() {
        let mut app = App::new(Metrics::new());
        app.push_log("noise");
        app.set_input("clear".to_string());
        assert!(matches!(app.submit(), Outcome::None));
        assert!(app.output.is_empty());
    }

    #[test]
    fn submit_help_sets_flag() {
        let mut app = App::new(Metrics::new());
        app.set_input("help".to_string());
        assert!(matches!(app.submit(), Outcome::None));
        assert!(app.show_help);
    }

    #[test]
    fn submit_wrfl_starts_write_flood() {
        let mut app = App::new(Metrics::new());
        app.set_input("wrfl -c 2 -r 5".to_string());
        match app.submit() {
            Outcome::StartFlood { write, params, label } => {
                assert!(write);
                assert_eq!(params.clients, 2);
                assert_eq!(params.requests, 5);
                assert_eq!(label, "wrfl -c 2 -r 5");
            }
            _ => panic!("expected StartFlood"),
        }
    }

    #[test]
    fn submit_quit_command() {
        let mut app = App::new(Metrics::new());
        app.set_input("quit".to_string());
        assert!(matches!(app.submit(), Outcome::Quit));
    }

    #[test]
    fn submit_unknown_logs_a_line() {
        let mut app = App::new(Metrics::new());
        let before = app.output.len();
        app.set_input("frobnicate".to_string());
        assert!(matches!(app.submit(), Outcome::None));
        assert_eq!(app.output.len(), before + 1);
        assert!(app.output.last().unwrap().contains("Unknown command"));
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = App::new(Metrics::new());
        assert!(matches!(app.handle_key(ctrl('c')), Outcome::Quit));
    }

    #[test]
    fn ctrl_d_quits() {
        let mut app = App::new(Metrics::new());
        assert!(matches!(app.handle_key(ctrl('d')), Outcome::Quit));
    }

    #[test]
    fn esc_only_closes_help_and_never_quits() {
        let mut app = App::new(Metrics::new());
        app.show_help = true;
        assert!(matches!(app.handle_key(key(KeyCode::Esc)), Outcome::None));
        assert!(!app.show_help);
        // With help closed, Esc is a no-op (does not quit).
        assert!(matches!(app.handle_key(key(KeyCode::Esc)), Outcome::None));
    }

    #[test]
    fn typing_and_cursor_editing() {
        let mut app = App::new(Metrics::new());
        typ(&mut app, "helo");
        assert_eq!(app.input, "helo");
        assert_eq!(app.cursor, 4);

        // Move left and insert a char in the middle.
        let _ = app.handle_key(key(KeyCode::Left));
        let _ = app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 4);

        // Backspace removes the char before the cursor.
        let _ = app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "helo");

        // Home + Delete removes the first char.
        let _ = app.handle_key(key(KeyCode::Home));
        let _ = app.handle_key(key(KeyCode::Delete));
        assert_eq!(app.input, "elo");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn tab_and_backtab_cycle_dashboard_tabs() {
        let mut app = App::new(Metrics::new());
        assert_eq!(app.active_tab, 0);

        // Tab advances and wraps around all four tabs.
        for expected in [1, 2, 3, 0] {
            let _ = app.handle_key(key(KeyCode::Tab));
            assert_eq!(app.active_tab, expected);
        }

        // Shift-Tab goes backward and wraps to the last tab.
        let _ = app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.active_tab, DashTab::ALL.len() - 1);
    }

    #[test]
    fn history_up_down_recalls_commands() {
        let mut app = App::new(Metrics::new());
        app.set_input("clear".to_string());
        let _ = app.submit();
        app.set_input("help".to_string());
        let _ = app.submit();

        // Up once -> most recent ("help"), again -> ("clear").
        let _ = app.handle_key(key(KeyCode::Up));
        assert_eq!(app.input, "help");
        let _ = app.handle_key(key(KeyCode::Up));
        assert_eq!(app.input, "clear");

        // Down steps forward, then back to an empty live line.
        let _ = app.handle_key(key(KeyCode::Down));
        assert_eq!(app.input, "help");
        let _ = app.handle_key(key(KeyCode::Down));
        assert_eq!(app.input, "");
    }
}
