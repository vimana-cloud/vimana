# (Re)start the local Kind cluster
# with freshly-built `vimanad` runtime and API operator installed.
# This can take a minute or two,
# so it may be faster to hotswap instead.

set -e
source 'dev/lib/util.sh'
assert-bazel-run

# Make sure that subscripts (i.e. the image push scripts)
# have access to the Bash runfiles library.
# https://github.com/bazelbuild/rules_shell/blob/v0.6.1/shell/runfiles/runfiles.bash
[ -z "$RUNFILES_DIR" ] && export RUNFILES_DIR="${BASH_SOURCE[0]}.runfiles"

# Standard K8s tool binaries:
kubectl="$1"
helm="$2"
kind="$3"
# Path to binaries that, when run,
# build and push various images to the local registry.
push_node_image="$4"
push_node_vimana_image="$5"
push_operator_image="$6"
# Full names (including registry and tag) of the node images.
node_image="$7"
node_vimana_image="$8"
# Probably `host.docker.internal:5000`.
cluster_registry="$9"
# Path to an executable that will install the Vimana APIs and operator in our cluster.
deploy_operator="${10}"
# Path to a local directory containing the Envoy Gateway helm chart.
envoy_gateway_helm_chart="${11}"
# Path to a local directory containing the `cert-manager` helm chart.
cert_manager_helm_chart="${12}"
# Path to the Kind cluster configuration file.
cluster_config="${13}"
shift 13

# Try to delete any existing cluster so we can start fresh.
"$kind" delete cluster 2> /dev/null || true
# Remove Docker's cached copies of the node images so Kind will re-pull them.
docker image rm --force "$node_image" 2> /dev/null || true
docker image rm --force "$node_vimana_image" 2> /dev/null || true

# Push the most up-to-date versions of locally-built images to the local registry.
# This includes the stock node image and the Vimana-enabled node image.
"$push_node_image" --insecure || {
  log-error 'Failed to push stock Kind node image'
  exit 1
}
"$push_node_vimana_image" --insecure || {
  log-error 'Failed to push Vimana-enabled Kind node image'
  exit 1
}
"$push_operator_image" --insecure || {
  log-error 'Failed to push operator image'
  exit 1
}

# Start Kind with the configured nodes.
"$kind" create cluster --config="$cluster_config" "$@"

# Patch kindnet (which requires an OCI runtime) so it will not run on Vimana nodes.
# The Vimana runtime uses the host network interface directly,
# so it does not need the bridged veth-pairs kindnet provides.
# Routing is done statically (see below).
#
# Kindnet tolerates all taints, so use nodeAffinity instead.
"$kubectl" patch daemonset kindnet -n kube-system --type=strategic -p '
spec:
  template:
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
            - matchExpressions:
              - key: runtime
                operator: NotIn
                values:
                - "vimanad"
'

echo >&2 -n 'Setting up host communication ...'
cluster_registry_directory="/etc/containerd/certs.d/${cluster_registry}"
for node in $("$kind" get nodes)
do
  # On Docker Desktop (Mac/Windows), `host.docker.internal` resolves automatically.
  # On Linux, it does not, so add it manually to `/etc/hosts` using the default gateway IP.
  # https://github.com/kubernetes-sigs/kind/blob/v0.31.0/images/base/files/usr/local/bin/entrypoint#L450-L456
  # TODO: Replace this if/when Kind allows configuring hosts more naturally:
  #   https://github.com/kubernetes-sigs/kind/issues/3282
  docker exec "$node" sh -c \
    'getent ahostsv4 host.docker.internal > /dev/null 2>&1 || \
      echo "$(ip -4 route show default | cut -d" " -f3) host.docker.internal" >> /etc/hosts'

  # Configure the cluster registry as "insecure".
  # https://kind.sigs.k8s.io/docs/user/local-registry/
  # TODO: Migrate to the "better UX" for this once implemented:
  #   https://github.com/kubernetes-sigs/kind/issues/1213
  docker exec "$node" mkdir -p "$cluster_registry_directory"
  cat <<EOF | docker exec -i "$node" cp /dev/stdin "${cluster_registry_directory}/hosts.toml"
[host."http://${cluster_registry}"]
EOF
done
echo >&2 ' Done'

# For each node, add static routes to all other nodes' pod CIDRs.
# This replaces kindnet, which can't run on Vimana-only nodes.
# See `docs/networking.md` for details.
echo >&2 -n 'Setting up static inter-node routing ...'
nodes=$("$kubectl" get nodes -o jsonpath='{range .items[*]}{.metadata.name} {.status.addresses[?(@.type=="InternalIP")].address} {.spec.podCIDR}{"\n"}{end}')
while IFS=' ' read -r src_node src_ip src_cidr
do
  while IFS=' ' read -r dst_node dst_ip dst_cidr
  do
    [ "$src_node" = "$dst_node" ] && continue
    docker exec "$src_node" ip route add "$dst_cidr" via "$dst_ip"
  done <<< "$nodes"
done <<< "$nodes"
echo >&2 ' Done'

echo >&2 'Waiting for all nodes to become ready ...'
"$kubectl" wait --for=condition=Ready nodes --all --timeout=120s
echo >&2 'Done'

# Install Envoy Gateway.
# This also sets up the Gateway API Custom Resource Definitions (CRDs).
# Use gateway namespace mode so that generated gateway services
# are in the same namespace as their `Gateway` resource:
# https://gateway.envoyproxy.io/docs/tasks/operations/gateway-namespace-mode/.
"$helm" install \
  envoy-gateway "$envoy_gateway_helm_chart" \
  --set=config.envoyGateway.provider.kubernetes.deploy.type=GatewayNamespace \
  --set=config.envoyGateway.extensionApis.enableEnvoyPatchPolicy=true \
  --namespace=envoy-gateway-system \
  --create-namespace

# Install `cert-manager` with Gateway API support
# so it can provision TLS certificates for Gateway listeners.
"$helm" install \
  cert-manager "$cert_manager_helm_chart" \
  --set=crds.enabled=true \
  --set=config.enableGatewayAPI=true \
  --namespace=cert-manager \
  --create-namespace

"$deploy_operator"
