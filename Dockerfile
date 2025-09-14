ARG TARGETPLATFORM=${TARGETPLATFORM}

FROM ubuntu:latest

COPY build/$TARGETPLATFORM/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
