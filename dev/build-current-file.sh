#!bash

# Take a source file path relative to the workspace directory,
# search for targets that build that file, and build them.
# Intended for use in IDE's, e.g. `.vscode/tasks.json`.

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

# Read all direct reverse dependencies in the same package (directory) as the source file
# into an array.
readarray -t targets < <(bazel query "same_pkg_direct_rdeps($1)")
target_count=${#targets[@]}

if (( target_count == 0 ))
then
  echo >&2 -e "${bold}${red}ERROR:$reset No targets in same package with direct dependency on $1"
  exit 1
else
  exec bazel build "${targets[@]}"
fi
