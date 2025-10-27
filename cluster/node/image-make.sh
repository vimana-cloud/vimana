# Create a node image for a Vimana cluster.

set -e
source 'dev/lib/util.sh'
assert-bazel-run

vimanad_binary_path="$1"
vimanad_service_path="$2"
containerd_config_path="$3"
shift 3

assert-command-available git

# If this script is run from an unmodified commit of the repository, it is considered clean.
if git -C "$BUILD_WORKSPACE_DIRECTORY" diff-index --quiet HEAD
then
  # During clean builds, the version of the image is the short form of the current commit hash.
  clean=1
  image_version="$(git -C "$BUILD_WORKSPACE_DIRECTORY" rev-parse --short HEAD)"
else
  # During dirty build, the version is just the current Unix time in seconds.
  clean=0
  image_version="$(date +%s)"
fi

function make-image-gcp {
  assert-command-available gcloud
  gcloud auth print-access-token --quiet > /dev/null 2> /dev/null || {
    log-error "Unauthenticated: run ${bold}gcloud auth login${reset}"
    exit 1
  }

  # These variables are non-local
  # because some of them are referenced by nested functions.
  # TODO: Do not publish this project name. Make it parameterized before open-sourcing.
  gcp_project='vimana-node-images'
  stock_image_project='debian-cloud'
  stock_image_family='debian-12'
  instance_name="image-dummy-${image_version}"
  instance_zone='us-west1-a'
  instance_type='e2-medium'
  snapshot_name="${instance_name}-snapshot"
  image_name="node-${image_version}"
  # Append `-dirty` to the image family if there are any uncommitted file changes.
  # That way, we can easily control whether to use only clean or dirty node images.
  image_family="vimana$((( clean )) || echo '-dirty')"

  log-info "Image name: ${bold}${image_name}${reset}"
  log-info "Image family: ${bold}${image_family}${reset}"

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

  local timeout=60
  log-info "Giving ${bold}${instance_name}${reset} up to $timeout seconds to become SSH-available"
  local start_time=$(date +%s)
  until gcloud compute ssh "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone" \
    --quiet \
    <<< exit 2> /dev/null
  do
    sleep 1s
    local current_time=$(date +%s)
    (( current_time - start_time > timeout )) && {
      log-error "Timed out after $timeout seconds"
      exit 1
    } || true  # The timeout check must have a successful status.
  done

  log-info "Uploading artifacts to ${bold}${instance_name}${reset}"
  gcloud compute scp \
    "$vimanad_binary_path" "$vimanad_service_path" "$containerd_config_path" \
    "$instance_name":'~/' \
    --project="$gcp_project" \
    --zone="$instance_zone"

  # SSH into the instance to:
  # - Move uploaded artifacts into proper directories (owned by root),
  #   which is more difficult to do directly via `scp`.
  # - Enable (but do not start) the `vimanad` daemon.
  # - Install [`cloud-init`](https://cloud-init.io/),
  #   which kOps expects to be enabled on the node image.
  # - Install `containerd` so kOps doesn't have to install it during node-up.
  log-info "Configuring ${bold}${instance_name}${reset}"
  gcloud compute ssh "$instance_name" \
    --project="$gcp_project" \
    --zone="$instance_zone" <<- EOF
      set -e
      sudo apt-get update
      sudo apt-get install -y cloud-init containerd
      sudo mv ~/'$(basename "$vimanad_binary_path")' /usr/bin/vimanad
      sudo mv ~/'$(basename "$vimanad_service_path")' /etc/systemd/system/vimanad.service
      sudo mv ~/'$(basename "$containerd_config_path")' /etc/containerd/config.toml
      sudo systemctl enable vimanad
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
    --family="$image_family" \
    --source-snapshot="$snapshot_name"

  log-info "Successfully created image ${bold}${image_name}${reset} under family ${bold}${image_family}${reset} 🙂"
}

# TODO: Also support other cloud platforms.
make-image-gcp
