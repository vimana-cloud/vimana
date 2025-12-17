#!/usr/bin/env bash

# Install the dependencies necessary to build and test Vimana
# from a fresh boot of the Debian "slim" container.

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install --yes curl jq git gcc g++ python3 uuid-runtime

# Install the latest version of Bazelisk from GitHub.
bazelisk_version="$(
  curl --silent --show-error --location \
    'https://api.github.com/repos/bazelbuild/bazelisk/releases/latest' \
      | jq --raw-output '.tag_name'
)"
bazelisk_package="bazelisk-$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/').deb"
curl --silent --show-error --location --remote-name \
  "https://github.com/bazelbuild/bazelisk/releases/download/${bazelisk_version}/${bazelisk_package}"
dpkg --install "${bazelisk_package}"
