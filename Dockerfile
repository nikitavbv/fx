FROM ubuntu:latest

ARG TARGETARCH

COPY build/$TARGETARCH/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
