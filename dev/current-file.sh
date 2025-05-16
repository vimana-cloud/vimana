#!/usr/bin/env bash

# Takes two positional arguments:
# 1. An action (either 'build' or 'test')
# 2. A source file path relative to the workspace directory
#
# Build all targets in the same package that directly depend on the given file.
#
# Test all test targets that are either:
# - in the same package and directly depend on the given file, or
# - in any package and directly depend on a
#   buildable rule in the same package that directly depends on the file.

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # https://en.wikipedia.org/wiki/ANSI_escape_code
  reset='\033[0m' # No formatting.
  bold='\033[1m'
  red='\033[1;31m'
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  red=''
fi

action="$1"
path="$2"

case "$action" in
  build)
    # Read all direct reverse dependencies in the same package (directory) as the source file
    # into an array.
    readarray -t targets < <(bazel query "same_pkg_direct_rdeps($path)")

    target_count=${#targets[@]}
    if (( target_count == 0 ))
    then
      echo >&2 -e \
        "${bold}${red}ERROR$reset No targets in same package with direct dependency on '$path'."
      exit 1
    fi

    exec bazel build "${targets[@]}"
    ;;
  test)
    # Select only the test targets
    # out of the union of the direct reverse dependencies
    # plus any target that directly depends on any of those.
    readarray -t targets < <( \
      bazel query \
        "tests(same_pkg_direct_rdeps($path) + rdeps(//..., same_pkg_direct_rdeps($path), 1))" \
    )

    target_count=${#targets[@]}
    if (( target_count == 0 ))
    then
      echo >&2 -e \
        "${bold}${red}ERROR$reset No test targets with semi-direct dependency on '$path'."
      exit 1
    fi

    exec bazel test "${targets[@]}"
    ;;
  *)
    echo >&2 -e "${bold}${red}ERROR$reset Unrecognized action '$action'."
    exit 1
    ;;
esac
