# Test-only image: the built rootfs plus python3-xlib, wlrctl, wtype, wev,
# wl-clipboard, and the Deskflow client with an X server of its own (xvfb)
# and xclip for the interop cases.
# Never shipped.
ARG BASE
FROM ${BASE}
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && \
    apt-get install -yq --no-install-recommends python3-xlib wlrctl wtype wev wl-clipboard deskflow xvfb xclip && \
    rm -rf /var/lib/apt/lists/*
COPY x11-probe.py /usr/local/bin/x11-probe.py
