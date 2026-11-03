# syntax=docker/dockerfile:1
# Orbbec-enabled a2a-lab-ot2 agent. This image does not contain robot-server.
# Ubuntu 24.04 matches the glibc used by the release Linux build.
FROM ubuntu:24.04

ARG TARGETARCH
COPY orbbec-sdk.env /tmp/orbbec-sdk.env

RUN export DEBIAN_FRONTEND=noninteractive \
  && apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl libusb-1.0-0 libudev1 \
  && . /tmp/orbbec-sdk.env \
  && case "$TARGETARCH" in \
       amd64) url="$ORBBEC_SDK_AMD64_URL"; sha="$ORBBEC_SDK_AMD64_SHA256" ;; \
       arm64) url="$ORBBEC_SDK_ARM64_URL"; sha="$ORBBEC_SDK_ARM64_SHA256" ;; \
       *) printf 'unsupported architecture %s; expected amd64 or arm64\n' "$TARGETARCH" >&2; exit 1 ;; \
     esac \
  && case "$url" in \
       https://github.com/orbbec/OrbbecSDK_v2/releases/download/v2.10.6/OrbbecSDK_v2.10.6_*_linux_*.tar.gz) ;; \
       *) printf 'refusing unpinned Orbbec SDK download %s\n' "$url" >&2; exit 1 ;; \
     esac \
  && curl -fsSL --retry 3 -o /tmp/orbbec.tar.gz "$url" \
  && echo "$sha  /tmp/orbbec.tar.gz" | sha256sum -c - \
  && mkdir -p /tmp/orbbec /opt/orbbec/lib /usr/share/doc/orbbec \
  && tar -xzf /tmp/orbbec.tar.gz -C /tmp/orbbec \
  && root="$(find /tmp/orbbec -mindepth 1 -maxdepth 1 -type d)" \
  && cp -a "$root/lib/." /opt/orbbec/lib/ \
  && rm -f /opt/orbbec/lib/*.cmake /opt/orbbec/lib/*.md \
  && cp "$root/LICENSE.txt" /usr/share/doc/orbbec/LICENSE.txt \
  && cp -a "$root/licenses" /usr/share/doc/orbbec/licenses \
  && cp "$root/shared/99-obsensor-libusb.rules" /usr/share/doc/orbbec/99-obsensor-libusb.rules \
  && rm -rf /tmp/orbbec /tmp/orbbec.tar.gz /tmp/orbbec-sdk.env \
  && apt-get purge -y curl \
  && apt-get autoremove -y \
  && rm -rf /var/lib/apt/lists/* \
  && test ! -e /usr/bin/gcc \
  && test ! -e /usr/bin/cc \
  && test ! -d /opt/orbbec/examples \
  && test ! -e /usr/local/bin/OrbbecViewer \
  && test ! -e /opt/opentrons \
  && test -f /usr/share/doc/orbbec/LICENSE.txt \
  && test -d /usr/share/doc/orbbec/licenses \
  && test -f /opt/orbbec/lib/libOrbbecSDK.so.2.10.6

COPY a2a-lab-ot2 /usr/local/bin/a2a-lab-ot2
RUN chmod 755 /usr/local/bin/a2a-lab-ot2 \
  && useradd --home-dir /var/lib/a2a-lab-ot2 --shell /usr/sbin/nologin a2alab \
  && mkdir -p /var/lib/a2a-lab-ot2 \
  && chown a2alab:a2alab /var/lib/a2a-lab-ot2 \
  && LD_LIBRARY_PATH=/opt/orbbec/lib /usr/local/bin/a2a-lab-ot2 orbbec-sdk-version | grep -qx '2.10.6'

ENV LD_LIBRARY_PATH=/opt/orbbec/lib \
    A2ALAB_ORBBEC_EXTENSIONS=/opt/orbbec/lib/extensions \
    A2ALAB_LISTEN_HOST=0.0.0.0 \
    A2ALAB_SILA_HOST=0.0.0.0
WORKDIR /var/lib/a2a-lab-ot2
VOLUME /var/lib/a2a-lab-ot2
USER a2alab
EXPOSE 31000 31001 31003 50052
ENTRYPOINT ["/usr/local/bin/a2a-lab-ot2"]
