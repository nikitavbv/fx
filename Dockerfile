FROM ubuntu:latest

ARG TARGETPLATFORM

COPY build/$TARGETPLATFORM/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
