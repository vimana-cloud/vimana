# Hot-swap a freshly-built copy of `vimanad` into a running Kind cluster,
# and re-deploy the API operator.
#
# CAUTION:
# This should not affect any running containers that use containerd,
# however it does forcibly shut down all running Vimana containers
# *without notifying the control plane*, which may cause strange behavior
# including disappeared pods getting replaced by the deployment controller.
# It's best to only run this script if there are no running Vimana containers!

set -e
source 'dev/lib/util.sh'
assert-bazel-run

# Make sure that subscripts (i.e. the image push scripts)
# have access to the Bash runfiles library.
# https://github.com/bazelbuild/rules_shell/blob/v0.6.1/shell/runfiles/runfiles.bash
[ -z "$RUNFILES_DIR" ] && export RUNFILES_DIR="${BASH_SOURCE[0]}.runfiles"

# Standard K8s tool binaries:
kind="$1"
kubectl="$2"
# Path to freshly-compiled `vimanad` binary.
vimanad="$3"
# Path to an exectuble that will push the operator image to the local registry.
push_operator_image="$4"
# Path to an executable that will install the Vimana APIs and operator in our cluster.
deploy_operator="$5"
shift 5

# If no Kind cluster is currently running, abort.
"$kind" get clusters 2> /dev/null | read || {
  log-error "Kind cluster is not running: try ${bold}bazel run //dev/kind:restart${reset}"
  exit 1
}

# Update the `vimanad` binary for each node that's currently running it.
for node in $("$kind" get nodes)
do
  if docker exec "$node" systemctl is-active --quiet vimanad
  then
    # Copy the new binary into the worker node, then restart the daemon.
    docker cp --follow-link "$vimanad" "$node":'/usr/local/bin/vimanad'
    docker exec "$node" systemctl restart vimanad
  fi
done

# Push the latest operator image and re-deploy it.
"$push_operator_image" --insecure || {
  log-error 'Failed to push operator image'
  exit 1
}
"$deploy_operator"
