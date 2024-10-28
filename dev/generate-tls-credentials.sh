# Generate an RSA private key and self-signed TLS certificate.

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

key_path="dev/self-signed.key"
cert_path="dev/self-signed.key"

# Generate a private key.
openssl genrsa -out "$key_path" && {
  echo >&2 -e "${green}Generated$reset ${bold}${key_path}$reset"
  # Generate the certificate signing request
  # and use it to generate the self-signed certificate.
  openssl req -new -key "$key_path" \
      -subj '/C=US/ST=Alaska/L=Talkeetna/O=Fake Corp/CN=Self-Signed Certificate' \
      | openssl x509 -req -signkey "$key_path" -out "$cert_path" && {
    echo >&2 -e "${green}Generated$reset ${bold}${cert_path}$reset"
  }
}

popd > /dev/null
