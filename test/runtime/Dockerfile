# Test-only image: the built rootfs plus python3-xlib, wlrctl, wtype, wev,
# wl-clipboard and the Deskflow client for the interop case.
# Never shipped.
ARG BASE
FROM ${BASE}
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && \
    apt-get install -yq --no-install-recommends python3-xlib wlrctl wtype wev wl-clipboard deskflow && \
    rm -rf /var/lib/apt/lists/*
COPY x11-probe.py /usr/local/bin/x11-probe.py
