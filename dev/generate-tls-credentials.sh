# Generate an environment file
# with an RSA private key and TLS certificate for `localhost`.

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # https://en.wikipedia.org/wiki/ANSI_escape_code
  reset='\033[0m' # No formatting.
  bold='\033[1m'
  red='\033[1;31m'
  green='\033[1;32m'
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  red=''
  green=''
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

# Path to the generated file and environment variable names.
env_path='dev/self-signed.env'
key_name='TLS_KEY'
cert_name='TLS_CERT'

# Generate a private key.
key="$(openssl genrsa)"
[ $? -eq 0 ] && {
  # Generate the certificate signing request
  # and use it to generate a self-signed certificate,
  # which is saved as an environment variable.
  cert="$(openssl req -new -key <(echo "$key") \
            -subj '/CN=localhost' \
            -addext 'subjectAltName = DNS:localhost' \
            | openssl x509 -req -key <(echo "$key") -copy_extensions copy)"
  [ $? -eq 0 ] && {
    # Finally, combine both into an environment file
    # that can be loaded by a local API server.
    # Note that `docker run --env-file` cannot handle PEM-encoded variables,
    # so they must be sourced natively in Bash (thus `export` is necessary):
    # https://github.com/moby/moby/issues/12997
    printf "export ${key_name}=%q\nexport ${cert_name}=%q\n" "$key" "$cert" > "$env_path"
    echo >&2 -e "${green}Generated$reset ${bold}${env_path}$reset"
  } || echo >&2 -e "${red}Failed$reset to generate certificate."
} || echo >&2 -e "${red}Failed$reset to generate private key."

popd > /dev/null
