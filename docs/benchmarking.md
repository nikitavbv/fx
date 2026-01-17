# benchmarking

test instance: Hetzner CCX23 (4 vCPU AMD, 16GB RAM), ubuntu 24.04

```
# first, install Rust:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

. "$HOME/.cargo/env"

# install wasm32-unknown-unknown target:
rustup target add wasm32-unknown-unknown

# install other build dependencies:
apt update
apt install just build-essential capnproto sqlite3 wrk

# clone fx repo:
git clone https://github.com/nikitavbv/fx.git

cd fx

# run server:
just fortunes-server

# then, in a different tmux tab, run the benchmark itself:
just fortunes-benchmark
```

## current results

```
4 threads and 256 connections
Thread Stats   Avg      Stdev     Max   +/- Stdev
  Latency    15.09ms    2.23ms  40.60ms   96.12%
  Req/Sec     4.26k   432.04     5.15k    92.00%
254646 requests in 15.03s, 327.85MB read
Requests/sec:  16943.57
Transfer/sec:     21.81MB
```
