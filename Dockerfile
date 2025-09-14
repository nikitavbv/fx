FROM ubuntu:latest
COPY build/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
