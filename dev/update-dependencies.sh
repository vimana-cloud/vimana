# Update all Bazel and Rust dependencies in a `MODULE.bazel` file
# based on information from Bazel Central Registry and crates.io,
# respectively.

set -e
source 'dev/lib/util.sh'
assert-bazel-run

buildozer="$(realpath $1)" # Get the absolute path so it works after changing directory.
shift 1

assert-command-available curl
assert-command-available tail
assert-command-available jq

# Move to the top level of the Git Repo for this function.
# The source repo becomes the working directory.
# Source files can be mutated, in contrast to Bazel's usual hermeticity.
# https://bazel.build/docs/user-manual#running-executables
pushd "$BUILD_WORKSPACE_DIRECTORY" > /dev/null

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
        log-info "${green}${name}${reset} ${red}${current_version}${reset} → ${cyan}${latest_version}${reset}"
        echo "replace version $current_version ${latest_version}|//MODULE.bazel:${name}"
      fi
    )
  done
)"

# $1 is a package name on crates.io.
# Print the folder part of the URL.
# https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
function crate-index-folder {
  local name="$1"
  if (( ${#name} <= 2 ))
  then
    echo "${#name}"
  elif (( ${#name} == 3))
  then
    echo "3/${name:0:1}"
  else
    echo "${name:0:2}/${name:2:2}"
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
        log-info "${yellow}${name}${reset} ${red}${current_version}${reset} → ${cyan}${latest_version}${reset}"
        echo "replace version $current_version ${latest_version}|//MODULE.bazel:%${line_number}"
      fi
    )
  done)"

# Run all updates in a single Buildozer command file.
echo -e "$bazel_updates\n$rust_updates" | "$buildozer" -f -

# Go back to the initial working directory, because why not.
popd > /dev/null
