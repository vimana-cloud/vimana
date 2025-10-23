# (Re)start the local minikube cluster
# with a freshly-built `workd` runtime installed.
# This can take a minute or two,
# so it may be faster to hotswap instead.

set -e
source 'dev/bash-util.sh'
assert-bazel-run

# Standard K8s tool binaries:
kubectl="$1"
helm="$2"
# Minikube is run through a wrapper (see `_minikube`).
minikube_wrapper="$3"
minikube_bin="$4"
# Path to a binary that, when run,
# builds and pushes the latest `workd`-enhanced Kicbase image
# to the registry where minikube will look for it.
push_kicbase_image="$5"
push_operator_image="$6"
# Full name (including registry) of the Kicbase image.
# Probably `localhost:5000/kicbase-workd:latest`.
kicbase_repo="$7"
# Probably `host.minikube.internal:5000`.
cluster_registry="$8"
# Path to an executable that will install the Vimana APIs and operator in our cluster.
deploy_operator="$9"
shift 9

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

# Push the most up-to-date versions of locally-built images to the local registry.
# This includes Vimana-enabled Kicbase and the operator image.
# This should be the command for `bazel run //dev/minikube:kicbase-image-push-local`
# and it should push to the same registry as `$kicbase_repo`.
"$push_kicbase_image" --insecure || {
  log-error "Failed to push ${bold}${kicbase_repo}${reset}"
  exit 1
}
"$push_operator_image" --insecure || {
  log-error 'Failed to push operator image'
  exit 1
}

# Start minikube with:
# - Custom base image enabling the Workd runtime.
# - The ability to load containers from the host machine without TLS.
# - The runtime class of the pod specified on container image pull requests:
#   https://kubernetes.io/docs/reference/command-line-tools-reference/feature-gates/
# - Embedding certificate data in the generated kubeconfig so it's self-contained.
_minikube start \
  --base-image="$kicbase_repo" \
  --container-runtime=workd \
  --insecure-registry="$cluster_registry" \
  --feature-gates=RuntimeClassInImageCriApi=true \
  --embed-certs \
  "$@"

# Enable all dashboard features.
_minikube addons enable metrics-server

# Install Envoy Gateway.
# This also sets up the Gateway API Custom Resource Definitions (CRDs).
# Use gateway namespace mode so that generated gateway services
# are in the same namespace as their `Gateway` resource:
# https://gateway.envoyproxy.io/docs/tasks/operations/gateway-namespace-mode/.
# TODO: Use `rules_oci` to download the chart and re-use the exact same version in prod.
"$helm" install \
  --set=config.envoyGateway.provider.kubernetes.deploy.type=GatewayNamespace \
  envoy-gateway 'oci://docker.io/envoyproxy/gateway-helm' \
  --version=v1.4.2 \
  --namespace=envoy-gateway-system \
  --create-namespace

"$deploy_operator"
