#!bash

# Move to the directory of this shell script,
# which should be the same directory as `package.json`.
pushd "$(dirname "$0")"

# Run NPM as normal from that directory,
# just downloaded through Bazel for convenience.
bazel run @nodejs//:npm -- install
bazel run @nodejs//:npm -- run docs:dev
