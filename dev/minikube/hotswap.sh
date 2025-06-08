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
  reset="$(tput sgr0)"
  bold="$(tput bold)"
  red="$(tput setaf 1)"
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

# Minikube is run through a wrapper (see `_minikube`).
minikube_wrapper="$1"
minikube_bin="$2"
kubectl="$3"
# Path to freshly-compiled `workd` binary.
workd="$4"

function _minikube {
  # Leaky abstraction :(
  # but this seems to be the only way
  # to get minikube to use the packaged kubectl binary.
  # See `@rules_k8s//:minikube`.
  "$minikube_wrapper" "$minikube_bin" "$kubectl" "$@"
}

# If minikube is not currently running, abort.
_minikube status > /dev/null 2> /dev/null || {
  echo >&2 -e "${red}Error$reset minikube is not running. Try ${bold}bazel run //dev/minikube:restart$reset"
  exit 1
}

# Hot-swapping is pretty simple actually.
# Just copy the new binary into kicbase, then restart the daemon.
docker cp --follow-link "$workd" minikube:/usr/bin/workd
docker exec minikube systemctl restart workd
