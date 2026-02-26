# Patch the CoreDNS Corefile.

# Patch CoreDNS' Corefile to include an etcd source,
# pointing to a dedicated etcd instance called 'cert-manager/etcd-server'.
# This works in concert with ExternalDNS,
# which will keep the etcd directory up-to-date in SkyDNS format
# according to declared DNSEndpoint resources.
#
# This is necessary for end-to-end testing
# because test domains must be reachable from Pebble
# in order to complete the ACME pre-challenge check and provision TLS certificates.

# https://github.com/bazelbuild/rules_shell/blob/v0.6.1/shell/runfiles/runfiles.bash#L70-L80
# --- begin runfiles.bash initialization v3 ---
# Copy-pasted from the Bazel Bash runfiles library v3.
set -uo pipefail; set +e; f=bazel_tools/tools/bash/runfiles/runfiles.bash
# shellcheck disable=SC1090
source "${RUNFILES_DIR:-/dev/null}/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "${RUNFILES_MANIFEST_FILE:-/dev/null}" | cut -f2- -d' ')" 2>/dev/null || \
  source "$0.runfiles/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.exe.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  { echo>&2 "ERROR: cannot find $f"; exit 1; }; f=; set -e
# --- end runfiles.bash initialization v3 ---

# Convert the `jq` path from exec to root form so it can be used from an executable rule.
jq="$(rlocation "${jq#external/}")"

# Look up the etcd server's ClusterIP so that CoreDNS can connect to it
# without needing to resolve a hostname via DNS (which it can't do for its own
# cluster-internal names during bootstrap).
etcd_ip="$("$kubectl" get service etcd-server --namespace='cert-manager' --output=jsonpath='{.spec.clusterIP}')" || {
  echo >&2 "Failed to look up etcd-server service ClusterIP in namespace 'cert-manager'."
  exit 1
}

# This clause will be inserted into the Corefile,
# right above the `kubernetes` plugin for cluster.local etc.
augmentation="    etcd {
        path /skydns
        endpoint http://${etcd_ip}:2379
        fallthrough
    }"

# The Corefile will be patched above the line that looks like this:
#     kubernetes cluster.local in-addr.arpa ip6.arpa {
trigger='^    kubernetes cluster.local .* {'

# Atomically patch the CoreDNS Corefile ConfigMap according to a patching function / command.
# The patching function must read the Corefile on stdin and write the updated Corefile to stdout.
#
# If the patching function returns a non-zero status code,
# leave the ConfigMap unchanged and return that same status code.
#
# Use `resourceVersion`-based optimistic concurrency with retries.
function corefile-patch {
  local patch="$1"
  local attempt=0
  local max_retries=3
  while (( attempt < max_retries ))
  do
    local config_map
    config_map="$(
      "$kubectl" --namespace='kube-system' get configmap coredns --output=json
    )" || return $?
    local corefile
    corefile="$(<<< "$config_map" "$jq" --raw-output '.data.Corefile')" || return $?
    corefile="$(<<< "$corefile" $patch)" || return $?
    local updated
    updated="$(<<< "$config_map" "$jq" --arg 'corefile' "$corefile" '.data.Corefile = $corefile')" \
      || return $?

    if "$kubectl" --namespace='kube-system' replace --filename=- <<< "$updated"
    then return 0
    fi

    (( attempt++ ))
    echo >&2 "CoreDNS ConfigMap replacement conflict (${attempt}/${max_retries})"
    sleep $attempt
  done
  echo >&2 "Failed to patch CoreDNS for end-to-end testing after $max_retries attempts"
  return 1
}

# Function which applies the augmentation
# to the Corefile contents read from stdin.
function corefile-augment {
  local corefile="$(cat)"

  if [[ "$corefile" == *"$augmentation"* ]]
  then
    echo >&2 "CoreDNS already patched for end-to-end testing"
    # Use this magic status code to indicate "all good".
    return 66
  fi

  # Insert the augmentation above the trigger line.
  # Build a sed script file so `i\` + continuation lines work on both GNU and BSD sed.
  local script="$(mktemp)"
  {
    printf '/%s/i\\\n' "$trigger"
    printf '%s\n' "$augmentation" | sed '$!s/$/\\/'
  } > "$script"
  <<< "$corefile" sed -f "$script"
  rm -f "$script"
}

corefile-patch corefile-augment || {
  status=$?
  # Check for the magic "all good" status code.
  [[ $status == 66 ]] || exit $status
}
