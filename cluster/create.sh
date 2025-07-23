#!/usr/bin/env bash

set -e
source 'dev/bash-util.sh'

kops="$1"
shift 1

function create-cluster-gcp {
  assert-command-available jq
  assert-command-available gcloud

  # "Regular" credentials are needed to query the latest image name.
  gcloud auth print-access-token --quiet > /dev/null 2> /dev/null || {
    log-error "Unauthenticated: run ${bold}gcloud auth login${reset}"
    exit 1
  }
  # Application default credentials are needed to run kOps.
  gcloud auth application-default print-access-token --quiet > /dev/null 2> /dev/null || {
    log-error "Unauthenticated: run ${bold}gcloud auth application-default login${reset}"
    exit 1
  }

  local cloud='gce'
  local gcp_project='foobar'
  local cluster_name='foo.bar'
  local kops_state_store='gs://foo-me-bars/'
  local zones='us-west4-a'
  local control_node_count='1'
  local control_machine_type='e2-medium'
  local work_node_count='2'
  local work_machine_type='e2-medium'
  local image_project='foo-fersher'
  local image_family='vimana-dirty' # TODO: Make it 'vimana'

  # Get the latest image within the image family.
  local image_json="$(
    gcloud compute images describe-from-family $image_family \
      --project=${image_project} \
      --format='json(name,creationTimestamp)' \
      2> /dev/null
  )"
  [ -n "$image_json" ] || {
    log-error "No images found under ${bold}${image_family}${reset} family in ${bold}${image_project}${reset}"
    log-error "Try running ${bold}bazel run //cluster/node:make-image${reset} first"
    exit 1
  }
  local image_name="${image_project}/$(<<< "$image_json" jq --raw-output '.name')"
  local image_create_time="$(<<< "$image_json" jq --raw-output '.creationTimestamp')"
  log-info "Using image ${bold}${image_name}${reset} created at ${bold}${image_create_time}${reset}"

  # TODO: Flesh out the list of necessary APIs.
  #gcloud services enable --project="$gcp_project" \
  #  iam.googleapis.com \
  #  cloudresourcemanager.googleapis.com

  # Make sure `<project-number>@cloudservices.gserviceaccount.com` ($gcp_project)
  # has `roles/compute.imageUser` on `projects/vimana-node-images`.

  "$kops" create cluster "$cluster_name" \
    --cloud="$cloud" \
    --project="$gcp_project" \
    --state="$kops_state_store" \
    --zones="$zones" \
    --control-plane-count="$control_node_count" \
    --control-plane-size="$control_machine_type" \
    --node-count="$work_node_count" \
    --node-size="$work_machine_type" \
    --image="${image_name}" \
    --networking='kube-router' \
    --kubernetes-feature-gates='+RuntimeClassInImageCriApi' \
    --set 'spec.containerd.skipInstall=true' \
    --set 'spec.containerd.address=/run/vimana/workd.sock' \
    --yes
    #--dry-run -o yaml
    #--topology='private' \
    #--bastion \
}

# TODO: Also support other cloud platforms.
create-cluster-gcp
