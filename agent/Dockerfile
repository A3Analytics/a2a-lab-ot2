# syntax=docker/dockerfile:1
# a2a-lab-ot2 agent. This image does not contain robot-server or a camera SDK.
# Ubuntu 24.04 matches the glibc used by the release Linux build.
FROM ubuntu:24.04

RUN export DEBIAN_FRONTEND=noninteractive \
  && apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && test ! -e /usr/bin/gcc \
  && test ! -e /usr/bin/cc \
  && test ! -d /opt/orbbec \
  && test ! -e /opt/opentrons

COPY a2a-lab-ot2 /usr/local/bin/a2a-lab-ot2
RUN chmod 755 /usr/local/bin/a2a-lab-ot2 \
  && useradd --home-dir /var/lib/a2a-lab-ot2 --shell /usr/sbin/nologin a2alab \
  && mkdir -p /var/lib/a2a-lab-ot2 \
  && chown a2alab:a2alab /var/lib/a2a-lab-ot2 \
  && /usr/local/bin/a2a-lab-ot2 --version >/dev/null

ENV A2ALAB_LISTEN_HOST=0.0.0.0 \
    A2ALAB_SILA_HOST=0.0.0.0
WORKDIR /var/lib/a2a-lab-ot2
VOLUME /var/lib/a2a-lab-ot2
USER a2alab
EXPOSE 31000 31001 31003 50052
ENTRYPOINT ["/usr/local/bin/a2a-lab-ot2"]
