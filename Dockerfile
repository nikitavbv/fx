FROM ubuntu:latest

ARG TARGETPLATFORM=${TARGETPLATFORM}

COPY build/$TARGETPLATFORM/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
