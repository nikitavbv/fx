FROM ubuntu:latest

RUN echo "do logs work" && env && exit -1

COPY build/$TARGETPLATFORM/fx-runtime /usr/local/bin/fx

ENTRYPOINT ["fx"]
