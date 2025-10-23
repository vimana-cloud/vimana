# Hot-swap a freshly-built copy of `workd` into a running minikube cluster,
# and re-deploy the API operator.
#
# CAUTION:
# This should not affect any running containers that use containerd,
# however it does forcibly shut down all running Vimana containers
# *without notifying the control plane*, which may cause strange behavior
# including disappeared pods getting replaced by the deployment controller.
# It's best to only run this script if there are no running Vimana containers!

set -e
source 'dev/bash-util.sh'
assert-bazel-run

# Minikube is run through a wrapper (see `_minikube`).
minikube_wrapper="$1"
minikube_bin="$2"
kubectl="$3"
# Path to freshly-compiled `workd` binary.
workd="$4"
push_operator_image="$5"
# Path to an executable that will install the Vimana APIs and operator in our cluster.
deploy_operator="$6"
shift 6

function _minikube {
  # Leaky abstraction :(
  # but this seems to be the only way
  # to get minikube to use the packaged kubectl binary.
  # See `@rules_k8s//:minikube`.
  "$minikube_wrapper" "$minikube_bin" "$kubectl" "$@"
}

# If minikube is not currently running, abort.
_minikube status > /dev/null 2> /dev/null || {
  log-error "Minikube is not running: try ${bold}bazel run //dev/minikube:restart${reset}"
  exit 1
}

# Hot-swapping is pretty simple actually.
# Just copy the new binary into kicbase, then restart the daemon.
docker cp --follow-link "$workd" minikube:/usr/bin/workd
docker exec minikube systemctl restart workd

# Push the latest operator image and re-deploy it.
"$push_operator_image" --insecure || {
  log-error 'Failed to push operator image'
  exit 1
}
"$deploy_operator"
