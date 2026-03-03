#!/usr/bin/env bash

# Deploy Pebble and force cert-manager to re-register with it.

"$kubectl" apply --filename="$pebble_resources"

# Pebble is an in-memory ACME test server that loses all account data on restart.
# Deleting this secret forces cert-manager to re-register with the current Pebble instance,
# avoiding issues with stale account state.
"$kubectl" delete secret pebble-account-key \
    --namespace=cert-manager \
    --ignore-not-found=true
