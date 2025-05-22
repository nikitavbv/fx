FROM alpine:latest
COPY target/x86_64-unknown-linux-musl/release/fx-cloud /usr/local/bin/fx
