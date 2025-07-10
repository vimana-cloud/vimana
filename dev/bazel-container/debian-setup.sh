#!/usr/bin/env bash

# Install all the dependencies required to build or test Vimana on Debian.

set -e

# Install basic dependencies.
apt-get update
apt-get install -y curl jq openssh-client git gcc g++ python3

# Install the latest release of Bazelisk from GitHub.
LATEST_BAZELISK="$(
  curl --silent --show-error https://api.github.com/repos/bazelbuild/bazelisk/releases/latest \
    | jq --raw-output '.tag_name'
)"
curl --silent --show-error --location --remote-name \
  "https://github.com/bazelbuild/bazelisk/releases/download/${LATEST_BAZELISK}/bazelisk-amd64.deb"
dpkg --install bazelisk-amd64.deb

# Run Bazelisk once to download the latest version of Bazel
# so we don't have to re-download it every time the Bazel container is used.
bazel > /dev/null
