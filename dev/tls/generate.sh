# Generate TLS credentials for use in a test environment,
# including a root CA certificate,
# and private keys and matching server certificates for various hostnames
# signed by that root CA.
#
#   .
#   ├── ROOT.key
#   ├── ROOT.cert
#   ├── localhost.key
#   ├── localhost.cert
#   └── <Maybe other key/certificate pairs...>

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

# Everything goes in the same directory.
out_dir='dev/tls'

# Print a success message about having generated the given file ($1).
function success-message {
  echo >&2 -e "${green}Generated$reset ${bold}${1}$reset"
}

# Print a failure message about having generated the given file ($1).
function failure-message {
  echo >&2 -e "${red}Failed$reset to generate ${bold}${1}$reset"
  false # Propagate the failure.
}

# Generate a private key and self-signed CA certificate
# with the given basename ($1).
function generate-ca-creds {
  key_path="${out_dir}/${1}.key"
  cert_path="${out_dir}/${1}.cert"

  openssl genrsa > "$key_path" && {
    success-message "$key_path"

    openssl req -new -key "$key_path" -subj "/CN=$1" \
      -addext 'keyUsage=critical,keyCertSign' \
      -addext 'basicConstraints=critical,CA:TRUE' \
      | openssl x509 -req -key "$key_path" \
        -copy_extensions copy \
        > "$cert_path" \
          && success-message "$cert_path" \
          || failure-message "$cert_path"
  } || failure-message "$key_path"
}

# Generate a private key and TLS certificate,
# signed by a CA certificate with the given basename ($1),
# for the given hostname ($2).
function generate-tls-creds {
  ca_key_path="${out_dir}/${1}.key"
  ca_cert_path="${out_dir}/${1}.cert"
  key_path="${out_dir}/${2}.key"
  cert_path="${out_dir}/${2}.cert"

  openssl genrsa > "$key_path" && {
    success-message "$key_path"

    openssl req -new -key "$key_path" -subj "/CN=$2" \
      -addext 'keyUsage=critical,keyEncipherment' \
      -addext "subjectAltName = DNS:$2" \
      | openssl x509 -req -CA "$ca_cert_path" -CAkey "$ca_key_path" \
        -copy_extensions copy \
        > "$cert_path" \
          && success-message "$cert_path" \
          || failure-message "$cert_path"
  } || failure-message "$key_path"
}

generate-ca-creds 'ROOT' && \
  generate-tls-creds 'ROOT' 'localhost'

popd > /dev/null
