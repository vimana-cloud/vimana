# A library of useful, general values and functions for Bash scripts.

# Bash scripts should pretty much always fail fast.
set -e

# Initialize a bunch of variables to hold text formatting escape sequences.
# https://en.wikipedia.org/wiki/ANSI_escape_code
function initialize-formatting {
  reset="$(tput sgr0)"
  bold="$(tput bold)"
  underline="$(tput smul)"
  red="$(tput setaf 1)"
  green="$(tput setaf 2)"
  yellow="$(tput setaf 3)"
  blue="$(tput setaf 4)"
  magenta="$(tput setaf 5)"
  cyan="$(tput setaf 6)"
}

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  initialize-formatting
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

# Log a message to stderr with the formatted prefix `[DEBUG] `.
function log-debug {
  local msg="$1"
  echo >&2 -e "[${magenta}DEBUG${reset}] $msg"
}

# Exit the script with an error message if the named command is not available on $PATH.
function assert-command-available {
  local command="$1"
  if ! command -v "$command" > /dev/null
  then
    log-error "${bold}${command}${reset} required but not found"
    return 1
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

# A modern version of getopts.
function parse-options {
  # The option and positional argument portions of the synopsis.
  local synopsis_options
  local synopsis_positionals
  # Array of lines describing each option for the help message.
  local -a option_descriptions=(
    "${bold}-h${reset}, ${bold}--help${reset}, ${bold}-?${reset}"
    '    Print this help message and exit.'
  )
  # Mapping from all option names to their canonical names.
  # Pre-populate the canonical mapping with built-in options
  # to trigger the uniqueness check if the user tries to configure one.
  local -A canonical=(
    ['-h']='-h'
    ['--help']='-h'
    ['-?']='-h'
  )
  # Mapping from canonical option names to parameter names.
  # If an option is not parameterized, the value will be empty.
  local -A parameters

  # Helper functions to build individual option descriptions for the help message.
  function ~describe-option~ {
    local parameter="$1"
    local option="$2"
    local separator=' '
    if [[ "$option" == --* ]]
    then
      separator='='
    fi
    echo -en "${bold}${option}${reset}${parameter:+${separator}${parameter}}"
  }
  function ~describe-options~ {
    local parameter="$1"
    local option="$2"
    shift 2
    ~describe-option~ "$parameter" "$option"
    for option in "$@"
    do
      echo -n ', '
      ~describe-option~ "$parameter" "$option"
    done
  }

  # Parse the option configuration.
  while [[ $# -gt 0 ]]
  do
    # Treat the rest of the arguments after the first `--` as inputs.
    if [[ "$1" == '--' ]]
    then
      shift
      break
    elif [[ "$1" =~ ^@(.*)$ ]]
    then
      synopsis_positionals=" [${bold}--${reset}] ${BASH_REMATCH[1]}"
    # Separate the option configuration into constituent parts:
    # - The option names, which cannot contain `:` or `~`.
    # - An optional parameter name following `:`
    # - An optional help message following `~`.
    elif [[ "$1" =~ ^([^:~]+)(:([^~]+))?(~(.*))?$ ]]
    then
      local names="${BASH_REMATCH[1]}"
      local parameter="${BASH_REMATCH[3]}"
      local help="${BASH_REMATCH[5]}"

      # Separate the names by commas.
      IFS=',' read -ra names <<< "$names"
      for name in "${names[@]}"
      do
        # Option names must be unique.
        if [[ -v "canonical[$name]" ]]
        then
          log-error "Non-unique option name: ${bold}${name}${reset}"
          return 1
        # Options are expected to conform to GNU-style syntax.
        elif ! [[ "$name" =~ ^(-.|--.+)$ ]]
        then
          log-error "Invalid option: ${bold}${name}${reset}"
          log-info "Options must match the regex ${bold}-.|--.+${reset}"
          return 1
        fi
        # The first variant is always the canonical form.
        canonical["$name"]="${names[0]}"
      done

      parameters["${names[0]}"]="$parameter"

      # Pre-populate the relevant elements of the help message.
      synopsis_options+=" [${bold}${names[0]}${reset}${parameter:+ ${parameter}}]"
      option_descriptions+=("$(~describe-options~ "$parameter" "${names[@]}")")
      if [[ -n "$help" ]]
      then
        while IFS='' read -r line
        do
          option_descriptions+=("    $line")
        done <<< "$help"
      fi
    else
      log-error "Invalid option configuration syntax: ${bold}${1}${reset}"
      return 1
    fi
    shift
  done

  # Helper function to print the help message to standard output.
  function ~print-help~ {
    echo "Usage: ${bold}${0}${reset}${synopsis_options}${synopsis_positionals}"
    # Only print the help description if it's not supplied interactively.
    if [ ! -t 0 ]
    then
      # If the input is empty, `read` will succeed for the first call, but supply an empty string.
      # Print nothing in that case.
      IFS='' read -r first_line
      if IFS='' read -r second_line
      then
        echo ''
        echo "$first_line"
        echo "$second_line"
        while IFS='' read -r line
        do
          echo "$line"
        done
      elif [[ -n "$first_line" ]]
      then
        echo ''
        echo "$first_line"
      fi
    fi
    echo ''
    echo 'Options:'
    for line in "${option_descriptions[@]}"
    do
      echo "    $line"
    done
  }

  # Clear the options map in case it's already populated with anything.
  unset options
  declare -gA options
  # Temporary storage for positional arguments that are intermingled with options.
  local -a positionals

  # Helper function to parse a single short or long option
  # where the parameter is space-delimited if present.
  function ~parse-option~ {
    if [[ -v "canonical[$1]" ]]
    then
      local option="${canonical["$1"]}"
      if [[ -n "${parameters["$option"]}" ]]
      then
        if [[ $# -lt 2 ]]
        then
          log-error "Missing argument for ${bold}${1}${reset}"
          ~print-help~ >&2
          exit 1
        fi
        options["$option"]="$2"
        # Indicate that an extra argument was consumed.
        return 1
      else
        # Set the option's value to an empty string to indicate presence
        # when the option doesn't take and argument.
        options["$option"]=''
        # Indicate that only a single argument was consumed.
        return 0
      fi
    else
      log-error "Unrecognized option: ${bold}${1}${reset}"
      ~print-help~ >&2
      exit 1
    fi
  }

  # If `errexit` is enabled, temporarily disable it.
  # The status code is used for control flow by `~parse-option~`.
  if [[ "$-" == *e* ]]
  then
    local errexit=1
    set +e
  fi

  # Parse the remaining options / arguments.
  while [[ $# -gt 0 ]]
  do
    # Treat the rest of the arguments after `--` as positional.
    if [[ "$1" == '--' ]]
    then
      shift
      break
    # `-h` and `--help` are hard-coded.
    elif [[ "$1" =~ ^(-h|--help|-\?)$ ]]
    then
      ~print-help~
      exit 0
    # Parse a GNU-style long option with an equals-delimited parameter.
    elif [[ "$1" =~ ^(--[^=]+)=(.*)$ ]]
    then
      if [[ -v "canonical[${BASH_REMATCH[1]}]" ]]
      then
        local option="${canonical[${BASH_REMATCH[1]}]}"
        if [[ -n "${parameters[$option]}" ]]
        then
          options["$option"]="${BASH_REMATCH[2]}"
        else
          log-error "Unexpected argument for ${bold}${BASH_REMATCH[1]}${reset}"
          ~print-help~ >&2
          exit 1
        fi
      else
        log-error "Unrecognized option: ${bold}${BASH_REMATCH[1]}${reset}"
        ~print-help~ >&2
        exit 1
      fi
    # Parse a GNU-style long option that may or may not be followed by an argument.
    elif [[ "$1" =~ ^(--.+)$ ]]
    then
      ~parse-option~ "${BASH_REMATCH[1]}" "${@:2}"
      shift $?
    elif [[ "$1" =~ ^-(.+)$ ]]
    then
      # Single-character options may be grouped.
      # Read one character at a time.
      while IFS='' read -rn 1 character
      do
        # `read -n 1` supplies an empty string at EOF.
        if [[ -z "$character" ]]
        then
          break
        fi
        ~parse-option~ "-${character}" "${@:2}"
        shift $?
      done <<< "${BASH_REMATCH[1]}"
    else
      positionals+=("$1")
    fi
    shift
  done

  # Restore `errexit` if it was temporarily disabled.
  if (( errexit ))
  then
    set -e
  fi

  # Consolidate the positional arguments.
  declare -ga arguments=("${positionals[@]}" "$@")
}

function assert-option-specified {
  local name="$1"
  if ! [[ -v "options[$name]" ]]
  then
    log-error "Missing required option: ${bold}${name}${reset}"
    return 1
  fi
}

function assert-option-conflict {
  local name="$1"
  local conflict="$2"
  if [[ -v "options[$name]" ]] && [[ -v "options[$conflict]" ]]
  then
    log-error "Option ${bold}${name}${reset} conflicts with ${bold}${conflict}${reset}"
    return 1
  fi
}
