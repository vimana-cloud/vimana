# (Re-)build the Vimana Bazel container from scratch (no caching).
# See `Dockerfile`.

set -e
source 'dev/bash-util.sh'
assert-bazel-run

dockerfile="$1"
debian_setup="$2"
shift 2

# Docker reguires that all files referenced by the Dockerfile (including the Dockerfile itself)
# exist beneath the "context" -- the directory in which `docker build` runs.
# Since Bazel may provide input files as symlinks to files in other directories,
# use a temporary directory to dereference the inputs and act as the context.
context="$(mktemp --directory)"
function cleanup-context {
  rm -rf "$context"
}
trap cleanup-context EXIT
cp -L "$dockerfile" "$debian_setup" "$context"

docker build --no-cache --tag vimana.host/bazel-container:latest "$context"
