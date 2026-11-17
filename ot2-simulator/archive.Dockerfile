# syntax=docker/dockerfile:1
ARG SIMULATOR_IMAGE
FROM ${SIMULATOR_IMAGE}

# The named context is a locally prepared bundle with a manifest and encoded media.
COPY --from=archive / /opt/ot2-replay/
ENV OT2_REPLAY_ARCHIVE=/opt/ot2-replay
