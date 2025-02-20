#!/usr/bin/env bash

# Hot-swap a freshly-built copy of `workd` into a running minikube cluster.
# This should not affect any running `kube-system` containers that use containerd,
# however it does forcibly shut down all running Vimana containers
# *without notifying the control plane* whatsoever, which may cause strange behavior
# including disappeared pods getting replaced by the deployment controller.

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # https://en.wikipedia.org/wiki/ANSI_escape_code
  reset='\033[0m' # No formatting.
  bold='\033[1m'
  red='\033[1;31m'
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  red=''
fi

# https://bazel.build/docs/user-manual#running-executables
if [ -z "$BUILD_WORKSPACE_DIRECTORY" ]
then
  echo >&2 -e "${red}Error$reset Run me with ${bold}bazel run$reset"
  exit 1
fi

# Path to minikube binary.
minikube="$1"
# Path to freshly-compiled `workd` binary.
workd="$2"
# Kicbase image name; probably `localhost:5000/kicbase-workd:latest`.
image_name="$3"

# If minikube is not currently running, abort.
"$minikube" status &>/dev/null || {
  echo >&2 -e "${red}Error$reset minikube is not running. Try ${bold}bazel run //dev/minikube:start$reset"
  exit 1
}

# Hot-swapping is pretty simple actually.
# Just copy the new binary into kicbase, then restart the daemon.
docker cp --follow-link "$workd" minikube:/usr/bin/workd
docker exec minikube systemctl restart workd
