# Yapper 🗣

A test / load client for [KurrentDB](https://www.kurrent.io/) (formerly EventStoreDB),
written in Rust with a [ratatui](https://ratatui.rs/) TUI.

Yapper can write, read, and subscribe to streams from the command line, run write/read
floods and **persistent-subscription load tests** to stress a node, and — in the TUI —
show a **live dashboard** of flood throughput/latency alongside the database's own server
stats (CPU, memory, disk IO, reader/writer queue peaks, persistent subscriptions, and TCP
connections) polled from KurrentDB's HTTP `/stats` endpoint.

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

Every data command runs in **single mode** by default (one client / subscriber).
Append `flood` to run **multiple concurrent clients**. `-c/--clients` is a
flood-only flag, so passing it in single mode is rejected — single mode is always
one client.

```sh
yapper config                                   # show current config + path
yapper write -s my-stream -t MyEvent -e '{"hello":"world"}'  # append one event
yapper read  -s my-stream                       # read one stream
yapper csub  -s my-stream                       # catch-up subscribe / live tail
yapper psub  -s my-stream -g my-group --create  # one persistent-subscription consumer

yapper write flood -c 4 -r 1000 -s 10 -p yap    # 4 clients, 1000 reqs, 10 streams each
yapper write flood -c 8 -d 30                    # sustained writes for 30s (-d defines the run)
yapper read  flood -c 4 -r 1000 -b 100          # 4 clients paging through $all
yapper csub  flood -n 4 -c 3 --create-streams --stream-length 50000 -d 120  # catch-up read-load
yapper psub  flood -n 4 -c 3 --create-streams --stream-length 50000 --ack-mode mix -d 120
yapper tui                                      # interactive TUI + live dashboard
```

For `write flood` / `read flood`, `-d/--duration` is optional and **defines how long the
flood runs**: with it the clients keep working for that many seconds (sustained load,
`--requests` is ignored); without it the flood stops once each client has done its
`--requests`. (This differs from `psub flood`, where `-d` is a drain *timeout* — see below.)

The **same commands work inside the TUI** — type them at the prompt to drive the
live dashboard. Pass `--config <path>` to use an alternate config file.

## Persistent subscription load testing

`yapper psub flood` load-tests persistent subscriptions. It creates one subscription
group per stream (`{prefix}{i}`) and runs a number of competing consumer clients per group:

```sh
yapper psub flood \
  -n 4 \                     # subscription groups, one per stream (yapper-ps-0 .. yapper-ps-3)
  -c 3 \                     # competing consumer clients per group
  --ack-mode mix \           # ack | nack | mix | none (what each client does per message)
  --nack-action park \       # park | retry | skip | stop (used by nack / mix)
  --create-streams \         # populate streams first if they are missing/empty
  --stream-length 50000 \    # events written per stream when creating
  -e 64 \                    # event payload size in bytes when creating
  -d 120                     # timeout: stop after 120s if not already drained (0 = no timeout)
```

The run **exits as soon as the streams are drained** — i.e. consumers have processed
everything and gone quiet — or when the `-d` timeout elapses, whichever comes first. With
`-d 0` there is no timeout, so it runs until the streams drain (or until you Ctrl-C). The
redelivery modes that never drain (`--ack-mode none`, or `nack` / `mix` with
`--nack-action retry`) keep messages flowing, so for those the timeout is what stops the run.

The target streams must **already be populated**. If a stream is missing or empty the run
aborts with a message unless you pass `--create-streams`, which writes `--stream-length`
events to each stream first so the subscribers have history to consume.

`--ack-mode` controls each consumer's per-message behaviour:

- `ack` — acknowledge every message (steady consumption).
- `nack` — negative-acknowledge every message, using `--nack-action`.
- `mix` — alternate ack / nack message by message.
- `none` — never settle messages, letting the server time them out and redeliver.

Consumers are unsubscribed and the groups are deleted on exit unless you pass `--keep`.
The single-consumer `yapper psub -s … -g …` tears its group down the same way.

Streams created by `--create-streams` are **kept** on a clean exit (so you can inspect or
re-run against them); pass `--delete-streams` to remove them on exit too, or `--keep` to
retain everything. A cancelled run always deletes what it created (unless `--keep`). The
same `--delete-streams` flag applies to `csub flood`.

## Testing

```sh
cargo test                  # hermetic unit tests (no database needed)
YAPPER_TEST_DB=1 cargo test  # also run the DB integration tests
```

Unit tests cover connection-string building + config (de)serialization, the metrics
percentile math, and the shared command grammar (`src/cli.rs`) that both front-ends
parse. The integration tests in `src/db.rs` self-skip unless `YAPPER_TEST_DB` is set,
and expect a node reachable via the default config (`127.0.0.1:2113`, insecure).

## TUI commands

Inside `yapper tui`, type commands at the prompt — they use the **same grammar** as the
CLI, so anything above works here too (`write flood -c 8`, `psub flood -n 4 …`, etc.):

- `clear` — clear the output
- `write` / `read` / `csub` / `psub` (each with optional `flood`) — run a job and drive
  the live dashboard; single-event reads and subscription events stream to the console
- `Esc` (or `stop` / `cancel`) — gracefully stop the running command
- `Ctrl+H` — toggle the help overlay (`Esc` also closes it), `Ctrl+C` / `Ctrl+D` — quit

Cancelling a command (via `Esc` / `stop` on the TUI, or `Ctrl+C` on the CLI) is
**graceful**: the run stops, and anything it created — persistent-subscription groups,
and streams populated by `--create-streams` — is deleted before it exits. Quitting the
TUI while a command runs cancels it the same way first.

Long-running jobs report their **stage** as they progress — on the CLI each stage prints
to stdout, and in the TUI it shows live in the Client panel's status line and is logged to
the console. For example a `psub flood` walks through `Checking streams… → Populating… →
Creating groups… → Subscribing N consumers… → Running… → Stopping consumers… → Deleting
groups…`.

While a job is running, a periodic progress line (`  N ops · R/s · E errors`, every ~2s)
is also logged — to stdout on the CLI and to the console in the TUI — so you can watch
throughput accrue alongside the live charts.
