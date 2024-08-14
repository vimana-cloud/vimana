# Update all Bazel and Rust dependencies in a `MODULE.bazel` file
# based on information from Bazel Central Registry and crates.io,
# respectively.
#
# Requires:
# - curl
# - jq
#
# Run this with `bazel run` as an `sh_binary` rule.
# Modifies `MODULE.bazel` in place.

# Terminal colors used for colored output.
red='\033[1;31m'
green='\033[1;32m'
brown='\033[1;33m'
blue='\033[1;34m'
reset='\033[0m' # No Color

# Move to the top level of the Git Repo for this function.
# This means the source repo becomes the working directory
# and source files can be mutated,
# in contrast to Bazel's usual hermeticity.
# https://bazel.build/docs/user-manual#running-executables
pushd "$BUILD_WORKSPACE_DIRECTORY" > /dev/null

# `$1` is the path to the `buildozer` executable, relative to `bazel-bin`.
# Use `bazel info` to construct an absolute path.
buildozer="$(bazel info bazel-bin)/$1" 

# Print the name and version of each `bazel_dep` in `MODULE.bazel`.
"$buildozer" "print name version" "//MODULE.bazel:%bazel_dep" |
  while read line
  do
    # Break each line into two space-delimited parts.
    [[ "$line" =~ ([^ ]+)\ ([^ ]+) ]] && (
      name="${BASH_REMATCH[1]}"
      current_version="${BASH_REMATCH[2]}"
      # Buildozer prints `(missing)` if the `bazel_dep` has no version.
      # It's an `extern/` dependency. Just skip these.
      [[ "$current_version" == "(missing)" ]] && exit

      # URL of metadata JSON file for this dependency in BCR.
      metadata_url="https://raw.githubusercontent.com/bazelbuild/bazel-central-registry/main/modules/$name/metadata.json"
      # Get the latest version from the registry.
      latest_version="$(curl --silent "$metadata_url" | jq --raw-output '.versions | last')"

      if [[ "$current_version" != "$latest_version" ]]
      then
        echo -e "$green$name$reset $red$current_version$reset → $blue$latest_version$reset"
        "$buildozer" "replace version $current_version $latest_version" "//MODULE.bazel:$name"
      fi
    )
  done

# https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
# $1 is a package name on crates.io.
function crate_index_folder {
  if (( ${#1} <= 2 )); then
    echo "${#1}"
  elif (( ${#1} == 3)); then
    echo "3/${1:0:1}"
  else
    echo "${1:0:2}/${1:2:2}"
  fi
}

# Print the line number, package name, and version of each `crate.spec` in `MODULE.bazel`.
# We need the line numbers for this one because they're technically not "rules",
# and can be referenced by line number, but not by name.
"$buildozer" "print startline package version" "//MODULE.bazel:%crate.spec" |
  while read line
  do
    # Break each line into three space-delimited parts.
    [[ "$line" =~ ([^ ]+)\ ([^ ]+)\ ([^ ]+) ]] && (
      line_number="${BASH_REMATCH[1]}"
      name="${BASH_REMATCH[2]}"
      current_version="${BASH_REMATCH[3]}"
      
      # URL of metadata JSON for this crate on crates.io.
      index_url="https://index.crates.io/$(crate_index_folder "$name")/$name"
      # Get the latest version from the index.
      latest_version="$(curl --silent "$index_url" | tail --lines=1 | jq --raw-output '.vers')"

      if [[ "$current_version" != "$latest_version" ]]
      then
        echo -e "$brown$name$reset $red$current_version$reset → $blue$latest_version$reset"
        "$buildozer" "replace version $current_version $latest_version" "//MODULE.bazel:%$line_number"
      fi
    )
  done

# Go back to the initial working directory, because why not.
popd > /dev/null
