# A library of useful, general values and functions for Bash scripts.

# Bash scripts should pretty much always fail fast.
set -e

# Initialize a bunch of variables to hold text formatting escape sequences.
# https://en.wikipedia.org/wiki/ANSI_escape_code
if [ -t 2 ]
then
  # Format output only if stderr (2) is a terminal (-t).
  reset="$(tput sgr0)"
  bold="$(tput bold)"
  red="$(tput setaf 1)"
  green="$(tput setaf 2)"
  yellow="$(tput setaf 3)"
  blue="$(tput setaf 4)"
  magenta="$(tput setaf 5)"
  cyan="$(tput setaf 6)"
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  red=''
  green=''
  yellow=''
  blue=''
  magenta=''
  cyan=''
fi

# Log a message to stderr with the formatted prefix `[ERROR] `.
function log-error {
  local msg="$1"
  echo >&2 -e "[${red}ERROR${reset}] $msg"
}

# Log a message to stderr with the formatted prefix `[WARN] `.
function log-warn {
  local msg="$1"
  echo >&2 -e "[${yellow}WARN${reset}] $msg"
}

# Log a message to stderr with the formatted prefix `[INFO] `.
function log-info {
  local msg="$1"
  echo >&2 -e "[${blue}INFO${reset}] $msg"
}

# Exit the script with an error message if the named command is not available on $PATH.
function assert-command-available {
  local command="$1"
  if ! which "$command" > /dev/null
  then
    log-error "${bold}${command}${reset} required but not found"
    exit 1
  fi
}

# Exit the script with an error message if it was invoked not invoked via `bazel run`.
# https://bazel.build/docs/user-manual#running-executables
function assert-bazel-run {
  if [ -z "$BUILD_WORKSPACE_DIRECTORY" ]
  then
    log-error "Run me with ${bold}bazel run${reset}"
    exit 1
  fi
}
