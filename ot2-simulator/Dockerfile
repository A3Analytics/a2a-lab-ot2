# syntax=docker/dockerfile:1
# Pinned OT-2 robot-server development simulator (Opentrons release tag only).
FROM python:3.12-slim-bookworm

ARG OPENTRONS_TAG=v10.0.0
RUN test -n "$OPENTRONS_TAG" \
  && test "$OPENTRONS_TAG" != edge \
  && test "$OPENTRONS_TAG" != latest \
  && test "$OPENTRONS_TAG" != main \
  && test "$OPENTRONS_TAG" != master

COPY --from=ghcr.io/astral-sh/uv:0.9.5 /uv /usr/local/bin/uv

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    gcc \
    git \
    build-essential \
    libsystemd-dev \
    pkg-config \
    systemd \
    bsdutils \
  && rm -rf /var/lib/apt/lists/*

ENV UV_LINK_MODE=copy \
    UV_PYTHON=/usr/local/bin/python3 \
    ENABLE_VIRTUAL_SMOOTHIE=true \
    OT_ROBOT_SERVER_DOT_ENV_PATH=/opt/opentrons/robot-server/dev.env \
    DEV_ROBOT_NAME=opentrons-dev

RUN git clone --depth 1 --branch "${OPENTRONS_TAG}" \
      https://github.com/Opentrons/opentrons.git /opt/opentrons

RUN python3 - /opt/opentrons/robot-server/pyproject.toml <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text()
old = 'exclude-newer = "1 week"\n'
if old in text:
    path.write_text(text.replace(old, "", 1))
PY

WORKDIR /opt/opentrons/robot-server
RUN uv sync --frozen --group dev --python /usr/local/bin/python3 \
  && uv pip uninstall --yes python-can >/dev/null 2>&1 || true

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 31950
ENTRYPOINT ["/entrypoint.sh"]
