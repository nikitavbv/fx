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
Running 1m test @ http://localhost:8080/fortunes
  4 threads and 10 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency   455.38us   76.76us   4.31ms   96.48%
    Req/Sec     4.40k   393.54     5.25k    83.44%
  1051683 requests in 1.00m, 1.32GB read
Requests/sec:  17499.09
Transfer/sec:     22.53MB
```

## comparing with axum

```
just fortunes-baseline-axum

# then, in a different tmux tab, run the benchmark itself:
just fortunes-benchmark
```

axum results:
```
Running 1m test @ http://localhost:8080/fortunes
  4 threads and 10 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency   221.16us  168.78us   4.18ms   88.31%
    Req/Sec     9.54k     1.96k   13.35k    78.73%
  2281525 requests in 1.00m, 2.87GB read
Requests/sec:  37962.62
Transfer/sec:     48.88MB
```

## comparing with nodejs

```
just fortunes-baseline-nodejs

# then, in a different tmux tab, run the benchmark itself:
just fortunes-benchmark
```

nodejs results:
```
Running 1m test @ http://localhost:8080/fortunes
  4 threads and 10 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency   465.46us  186.65us   4.17ms   82.20%
    Req/Sec     4.33k     0.94k    9.55k    70.77%
  1033741 requests in 1.00m, 1.36GB read
Requests/sec:  17200.48
Transfer/sec:     23.13MB
```
