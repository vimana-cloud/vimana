# Run this with `bazel run` as an `sh_binary` rule.
# Modifies the source tree in-place, adding a `node_modules/` directory :(.
#
# Run a VitePress dev server.
#
# Bazel's hermetic rules don't work well with VitePress' dev server,
# which watches the entire source tree and hot-reloads on changes.
# So we use Bazel just to download npm,
# then use npm like normal to download and run VitePress.
#
# Arguments:
# - Path to the `npm` executable.

set -e
source 'dev/bash-util.sh'
assert-bazel-run

npm="$(realpath $1)" # Get the absolute path so it works after changing directory.
shift 1

# Move to the directory of this shell script,
# which should be the same directory as the VitePress site's `package.json`
# and is also where `node_modules/` will be downloaded.
pushd "$BUILD_WORKSPACE_DIRECTORY/docs" > /dev/null

"$npm" install
"$npm" run docs:dev

popd > /dev/null
