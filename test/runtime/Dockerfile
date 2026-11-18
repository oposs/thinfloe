# Test-only image: the built rootfs plus python3-xlib, wlrctl, wtype and wev.
# Never shipped.
ARG BASE
FROM ${BASE}
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && \
    apt-get install -yq --no-install-recommends python3-xlib wlrctl wtype wev && \
    rm -rf /var/lib/apt/lists/*
COPY x11-probe.py /usr/local/bin/x11-probe.py
