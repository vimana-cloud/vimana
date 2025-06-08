# Update all Bazel and Rust dependencies in a `MODULE.bazel` file
# based on information from Bazel Central Registry and crates.io,
# respectively.
#
# Arguments:
# - Path to the `buildozer` executable.
#
# Requires:
# - curl
# - tail
# - jq

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # https://en.wikipedia.org/wiki/ANSI_escape_code
  reset="$(tput sgr0)"
  bold="$(tput bold)"
  red="$(tput setaf 1)"
  green="$(tput setaf 2)"
  yellow="$(tput setaf 3)"
  blue="$(tput setaf 4)"
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  red=''
  green=''
  yellow=''
  blue=''
fi

# Move to the top level of the Git Repo for this function.
# The source repo becomes the working directory.
# Source files can be mutated, in contrast to Bazel's usual hermeticity.
# https://bazel.build/docs/user-manual#running-executables
if [ -z "$BUILD_WORKSPACE_DIRECTORY" ]
then
  echo >&2 -e "${red}Error$reset Run me with ${bold}bazel run$reset"
  exit 1
fi
pushd "$BUILD_WORKSPACE_DIRECTORY" > /dev/null

buildozer="$1"

# The following creates a Buildozer command file to run a batch of commands together,
# storing the contents in a variable.
# Start by reading the name and version of each `bazel_dep` in `MODULE.bazel`.
bazel_updates="$("$buildozer" 'print name version' '//MODULE.bazel:%bazel_dep' |
  while read line
  do
    # Break each line into two space-delimited parts.
    [[ "$line" =~ ([^ ]+)\ ([^ ]+) ]] && (
      name="${BASH_REMATCH[1]}"
      current_version="${BASH_REMATCH[2]}"

      # Buildozer prints `(missing)` if the `bazel_dep` has no version.
      # It's an `extern/` dependency. Just skip these.
      [[ "$current_version" == '(missing)' ]] && exit

      # URL of metadata JSON file for this dependency in BCR.
      metadata_url="https://raw.githubusercontent.com/bazelbuild/bazel-central-registry/main/modules/$name/metadata.json"
      # Get the latest version from the registry.
      latest_version="$(curl --silent "$metadata_url" | jq --raw-output '.versions | last')"

      if [[ "$current_version" != "$latest_version" ]]
      then
        echo >&2 -e "$green$name$reset $red$current_version$reset → $blue$latest_version$reset"
        echo "replace version $current_version $latest_version|//MODULE.bazel:$name"
      fi
    )
  done
)"

# $1 is a package name on crates.io.
# Print the folder part of the URL.
# https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
function crate-index-folder {
  if (( ${#1} <= 2 ))
  then
    echo "${#1}"
  elif (( ${#1} == 3))
  then
    echo "3/${1:0:1}"
  else
    echo "${1:0:2}/${1:2:2}"
  fi
}

# The following creates a Buildozer command file to run a batch of commands together,
# storing the contents in a variable.
# Start by reading the line number, package name, and version
# of each `crate.spec` in `MODULE.bazel`.
# We need line numbers for this one because they're technically not "rules",
# and cannot be referenced by name, but can be referenced by line number.
rust_updates="$("$buildozer" "print startline package version" "//MODULE.bazel:%crate.spec" |
  while read line
  do
    # Break each line into three space-delimited parts.
    [[ "$line" =~ ([^ ]+)\ ([^ ]+)\ ([^ ]+) ]] && (
      line_number="${BASH_REMATCH[1]}"
      name="${BASH_REMATCH[2]}"
      current_version="${BASH_REMATCH[3]}"

      # URL of metadata JSON for this crate on crates.io.
      index_url="https://index.crates.io/$(crate-index-folder "$name")/$name"
      # Get the latest version from the index.
      latest_version="$(curl --silent "$index_url" | tail --lines=1 | jq --raw-output '.vers')"

      if [[ "$current_version" != "$latest_version" ]]
      then
        echo >&2 -e "$yellow$name$reset $red$current_version$reset → $blue$latest_version$reset"
        echo "replace version $current_version $latest_version|//MODULE.bazel:%$line_number"
      fi
    )
  done)"

# Run all updates in a single Buildozer command file.
echo -e "$bazel_updates\n$rust_updates" | "$buildozer" -f -

# Go back to the initial working directory, because why not.
popd > /dev/null
