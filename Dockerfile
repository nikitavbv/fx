FROM ubuntu:latest

RUN env

COPY build/$TARGETPLATFORM/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
