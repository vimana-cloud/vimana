#!/usr/bin/env bash

# Unlike most Bash scripts, this one is not meant to be invoked by Bazel.
# It is usually invoked directly by the IDE to perform some action on a particular file.
#
# Actions:
# - Build all targets in the same package that directly depend on the given file.
# - Test all test targets that are either:
#   * in the same package and directly depend on the given file, or
#   * in any package and directly depend on a
#     buildable rule in the same package that directly depends on the file.

set -e
source 'dev/bash-util.sh'

action="$1"  # Either 'build' or 'test'.
path="$2"    # Source file path relative to the workspace directory.

case "$action" in
  build)
    # Read all direct reverse dependencies in the same package (directory) as the source file
    # into an array.
    readarray -t targets < <(bazel query "same_pkg_direct_rdeps($path)")

    target_count=${#targets[@]}
    if (( target_count == 0 ))
    then
      log-error "No targets in same package with direct dependency on ${bold}${path}${reset}"
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
      log-error "No test targets with semi-direct dependency on ${bold}${path}${reset}"
      exit 1
    fi

    exec bazel test "${targets[@]}"
    ;;
  *)
    log-error "Unrecognized action ${bold}${action}${reset}"
    exit 1
    ;;
esac
