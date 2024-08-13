#!bash

# Bazel's hermetic rules don't work well for VitePress' dev server,
# which watches an entire source directory and hot-reloads on all changes.
# So we use Bazel just to download the `npm` executable,
# then use it like normal (it will create a `node_modules/` directory, etc.).

# Move to the directory of this shell script,
# which should be the same directory as the VitePress site's `package.json`
# and is also where `node_modules/` will be downloaded.
pushd "$(dirname "$0")" > /dev/null

bazel run @nodejs//:npm -- install
bazel run @nodejs//:npm -- run docs:dev

popd > /dev/null
