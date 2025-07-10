# Produce a simple Docker image capable of building Vimana
# on any platform that supports Docker.
#
# Using a Dockerfile in a Bazel project is discouraged:
# https://github.com/bazel-contrib/rules_oci/blob/v2.2.6/examples/dockerfile/README.md.
# We're ignoring this sage advice because:
# - This image is intended only for local development, so we're not worried about RBE.
# - It's a reproducibility improvement over the alternative (full multi-platform build support).
# - It's based on Debian -- a painfully stable distribution.
# - It's dead simple, installing only Bazelisk and a few well-exercised Debian packages.

FROM debian:stable-slim
COPY debian-setup.sh /debian-setup.sh
RUN /debian-setup.sh
