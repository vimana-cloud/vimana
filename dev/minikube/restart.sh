# (Re)start the local minikube cluster
# with a freshly-built `workd` runtime installed.
# This can take a minute or two,
# so it may be faster to hotswap instead.

set -e
source 'dev/bash-util.sh'
assert-bazel-run

# Standard K8s tool binaries:
kubectl="$1"
istioctl="$2"
# Minikube is run through a wrapper (see `_minikube`).
minikube_wrapper="$3"
minikube_bin="$4"
# Path to a binary that, when run,
# builds and pushes the latest `workd`-enhanced Kicbase image
# to the registry where minikube will look for it.
push_kicbase="$5"
# Probably `localhost:5000/kicbase-workd:latest`.
kicbase_repo="$6"
# Probably `host.minikube.internal:5000`.
cluster_registry="$7"
shift 7

function _minikube {
  # Leaky abstraction :(
  # but this seems to be the only way
  # to get minikube to use the packaged kubectl binary.
  # See `@rules_k8s//:minikube`.
  "$minikube_wrapper" "$minikube_bin" "$kubectl" "$@"
}

# Try to delete any running minikube cluster so we can start fresh.
_minikube delete
# If there is a running container called "minikube",
# remove it so minikube doesn't get confused.
docker rm minikube 2> /dev/null || true
# Finally, remove the Docker's cached copy of the kicbase image
# so minikube will re-pull it from the local registry.
docker image rm --force "$kicbase_repo" 2> /dev/null || true

# Push the most up-to-date version of Vimana-enabled Kicbase to the local registry.
# This should be the command for `bazel run //dev/minikube:kicbase-image-push-local`
# and it should push to the same registry as `$kicbase_repo`.
"$push_kicbase" --insecure || {
  log-error "Failed to push ${bold}${kicbase_repo}${reset}"
  exit 1
}

# Start minikube with:
# - Custom base image enabling the Workd runtime.
# - The ability to load containers from the host machine without TLS.
# - The runtime class of the pod specified on container image pull requests:
#   https://kubernetes.io/docs/reference/command-line-tools-reference/feature-gates/
# - Enough resources to run Istio: https://istio.io/latest/docs/setup/platform-setup/minikube.
# - Embedding certificate data in the generated kubeconfig so it's self-contained.
_minikube start \
  --base-image="$kicbase_repo" \
  --container-runtime=workd \
  --insecure-registry="$cluster_registry" \
  --feature-gates=RuntimeClassInImageCriApi=true \
  --memory=16384 --cpus=4 \
  --embed-certs \
  || exit 1

# Enable all dashboard features.
_minikube addons enable metrics-server

# Start Istio in ambient mode (no sidecars).
"$istioctl" install --skip-confirmation --set profile=ambient || exit 1

# Set up the Getway API Custom Resource Definitions (CRDs):
# https://github.com/kubernetes-sigs/gateway-api/releases.
"$kubectl" apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.1/standard-install.yaml || exit 1

# Set up the Runtime Class Manager.
#bazel run @runtime-class-manager//cmd/rcm:push-image-local || exit 1
#bazel run @runtime-class-manager//:install || exit 1
#bazel run @runtime-class-manager//:deploy "${cluster_registry}/runtime-class-manager:latest" || exit 1
