FROM ubuntu:latest

ARG TARGETARCH

COPY build/$TARGETARCH/fx-server /usr/local/bin/fx
RUN chmod +x /usr/local/bin/fx

ENTRYPOINT ["/usr/local/bin/fx"]
