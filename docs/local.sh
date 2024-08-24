# Run this with `bazel run` as an `sh_binary` rule.
# Modifies the source tree in-place, adding a `node_modules/` directory, etc. :(
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

# Use colored output if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # Terminal escape codes used for formatted output.
  reset='\033[0m' # No formatting.
  bold='\033[1m'
  red='\033[1;31m'
else
  # Make them all empty (no color) if we're printing to a pipe.
  reset=''
  bold=''
  red=''
fi

if [ -z "$BUILD_WORKSPACE_DIRECTORY" ]
then
  echo >&2 -e "${red}Error${reset} Run me with ${bold}bazel run$reset"
  exit 1
fi

# Move to the directory of this shell script,
# which should be the same directory as the VitePress site's `package.json`
# and is also where `node_modules/` will be downloaded.
pushd "$BUILD_WORKSPACE_DIRECTORY/docs" > /dev/null

# `$1` is relative to the rule's runfiles. Use `bazel info` to make an absolute path.
npm="$(bazel info bazel-bin)/docs/dev.runfiles/$1" 

"$npm" install
"$npm" run docs:dev

popd > /dev/null
