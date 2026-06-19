# Yapper 🗣

A test / load client for [KurrentDB](https://www.kurrent.io/) (formerly EventStoreDB),
written in Rust with a [ratatui](https://ratatui.rs/) TUI.

Yapper can write, read, and subscribe to streams from the command line, run write/read
floods to load-test a node, and — in the TUI — show a **live dashboard** of flood
throughput/latency alongside the database's own server stats (CPU, memory, disk IO,
queue lengths) polled from KurrentDB's HTTP `/stats` endpoint.

## Build

```sh
cargo build --release
```

## Run a local KurrentDB

```sh
docker run -d --name kurrentdb -p 2113:2113 \
  docker.kurrent.io/kurrent-latest/kurrentdb:latest \
  --insecure --run-projections=All --enable-atom-pub-over-http
```

The default config (`~/.yapper.json`, created on first run) targets `127.0.0.1:2113`
with TLS disabled and `admin`/`changeit`.

## Usage

```sh
yapper config                                   # show current config + path
yapper write single -s my-stream -t MyEvent -e '{"hello":"world"}'
yapper read   single -s my-stream
yapper write  flood  -c 4 -r 1000 -s 10 -p yap  # 4 clients, 1000 reqs, 10 streams each
yapper read   flood  -c 4 -r 1000 -p yap
yapper subscribe catchup    -s my-stream
yapper subscribe persistent -s my-stream -g my-group --create
yapper tui                                      # interactive TUI + live dashboard
```

Pass `--config <path>` to use an alternate config file.

## Testing

```sh
cargo test                  # hermetic unit tests (no database needed)
YAPPER_TEST_DB=1 cargo test  # also run the DB integration tests
```

Unit tests cover connection-string building + config (de)serialization, the metrics
percentile math, and the TUI command/flood-flag parsing. The integration tests in
`src/db.rs` self-skip unless `YAPPER_TEST_DB` is set, and expect a node reachable via
the default config (`127.0.0.1:2113`, insecure).

## TUI commands

Inside `yapper tui`, type commands at the prompt:

- `help` — show available commands
- `clear` — clear the output
- `wrfl ...` / `rdfl ...` — run a write/read flood and open the live dashboard
- `Esc` / `Ctrl+D` — back / close help, `Ctrl+C` — quit
