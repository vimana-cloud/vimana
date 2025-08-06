# See documentation on the associated Bazel rule.

set -e
source 'dev/bash-util.sh'
assert-bazel-run

kustomize="$(realpath "$1")"
image_name_and_tag="$2"
shift 2

cd "$BUILD_WORKSPACE_DIRECTORY"/operator/config/manager
exec "$kustomize" edit set image controller="$image_name_and_tag"
