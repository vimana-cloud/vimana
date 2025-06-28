#!/usr/bin/env bash

set -e

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # https://en.wikipedia.org/wiki/ANSI_escape_code
  reset="$(tput sgr0)"
  bold="$(tput bold)"
  red="$(tput setaf 1)"
  yellow="$(tput setaf 3)"
  blue="$(tput setaf 4)"
  magenta="$(tput setaf 5)"
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  red=''
  yellow=''
  blue=''
  magenta=''
fi

function log-info {
  echo >&2 -e "[${blue}INFO${reset}] $1"
}

function log-error {
  echo >&2 -e "[${red}ERROR${reset}] $1"
}

workd_binary_path="$1"
workd_service_path="$2"

# Move to the top level of the Git Repo to get the current commit hash.
# https://bazel.build/docs/user-manual#running-executables
[ -z "$BUILD_WORKSPACE_DIRECTORY" ] && {
  log-error "Run me with ${bold}bazel run$reset"
  exit 1
}
which git > /dev/null || {
  log-error "${bold}git${reset} required but not found"
  exit 1
}
pushd "$BUILD_WORKSPACE_DIRECTORY" > /dev/null
# The version of the image is the short form of the commit hash,
# possibly appended with `-dirty` if there are any uncommitted file changes.
image_version="$(git rev-parse --short HEAD)$(git diff-index --quiet HEAD || echo '-dirty')"
popd > /dev/null

function make-image-gcp {
  which gcloud > /dev/null || {
    log-error "${bold}gcloud${reset} required but not found"
    exit 1
  }
  gcloud auth print-access-token --quiet > /dev/null 2> /dev/null || {
    log-error "Unauthenticated: run ${bold}gcloud auth login${reset}"
    exit 1
  }

  gcp_project='vimana-node-images'
  stock_image_project='debian-cloud'
  stock_image_family='debian-12'
  instance_name="image-dummy-${image_version}"
  instance_zone='us-west1-a'
  instance_type='e2-medium'
  snapshot_name="${instance_name}-snapshot"
  image_name="vimana-${image_version}"

  log-info "Creating instance ${bold}${instance_name}${reset} from ${bold}${stock_image_project}/${stock_image_family}${reset}"
  gcloud compute instances create "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone" \
    --image-project="$stock_image_project" \
    --image-family="$stock_image_family" \
    --machine-type="$instance_type"

  # Clean up before exiting.
  function cleanup-instance {
    log-info "Deleting instance ${bold}${instance_name}${reset}"
    gcloud compute instances delete "$instance_name" \
      --project="$gcp_project" \
      --zone="$instance_zone" \
      --quiet
  }
  trap cleanup-instance EXIT

  timeout=60
  log-info "Giving ${bold}${instance_name}${reset} $timeout seconds to become SSH-available"
  start_time=$(date +%s)
  until gcloud compute ssh "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone" \
    --quiet 2> /dev/null <<< exit
  do
    sleep 1s
    current_time=$(date +%s)
    (( current_time - start_time > timeout )) && {
      log-error "Timed out after $timeout seconds"
      exit 1
    } || true  # The timeout check must have a successful status.
  done

  log-info "Uploading artifacts to ${bold}${instance_name}${reset}"
  gcloud compute scp "$workd_binary_path" "$workd_service_path" "$instance_name":'~/' \
    --project="$gcp_project" \
    --zone="$instance_zone"

  # SSH into the instance to:
  # - Move uploaded artifacts into proper directories (owned by root),
  #   which is more difficult to do directly via `scp`.
  # - Enable (but do not start) the `workd` daemon.
  # - Install [`cloud-init`](https://cloud-init.io/),
  #   which kOps expects to be enabled on the node image.
  log-info "Configuring ${bold}${instance_name}${reset}"
  gcloud compute ssh "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone" <<- EOF
      set -e
      sudo mv ~/'$(basename "$workd_binary_path")' /usr/bin/workd
      sudo mv ~/'$(basename "$workd_service_path")' /etc/systemd/system/workd.service
      sudo apt-get update
      sudo apt-get install -y cloud-init
      sudo systemctl enable workd
EOF

  log-info "Stopping ${bold}${instance_name}${reset} to preserve disk integrity during snapshot"
  gcloud compute instances stop "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone"

  log-info "Creating snapshot ${bold}${snapshot_name}${reset}"
  gcloud compute disks snapshot "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone" \
    --snapshot-names="$snapshot_name"

  # Clean up before exiting.
  function cleanup-snapshot-and-instance {
    log-info "Deleting snapshot ${bold}${snapshot_name}${reset}"
    gcloud compute snapshots delete "$snapshot_name" \
      --project="$gcp_project" \
      --quiet
    cleanup-instance
  }
  trap cleanup-snapshot-and-instance EXIT

  log-info "Creating image ${bold}${image_name}${reset} from the snapshot"
  gcloud compute images create "$image_name" \
    --project="$gcp_project" \
    --source-snapshot="$snapshot_name"

  log-info "Successfully created image ${bold}${image_name}${reset} 🙂"
}

# TODO: Also support other cloud platforms.
make-image-gcp
