# Viman API V1

All user interactions with the platform happen through the API.

## Test Credentials

The API server always uses HTTPS; never HTTP.
This requires *a bit* of extra setup in testing environments.

1. Generate TLS credentials for testing.
   They expire after 30 days by default,
   so you only need to run this once a month.
   ```bash
   bazel run //dev/tls:generate
   ```
2. **Optional:** import the generated root CA
   if you want to avoid dealing with untrusted certificates.
   For example, to install it dynamically (so it's automatically updated on regeneration)
   system-wide (for most Linux systems), run:
   ```bash
   sudo ln -s "${PWD}/dev/tls/ROOT.cert" /etc/ssl/certs/vimana-test.pem
   ```
   Importing may be different for various systems (MacOS) or various apps (like browsers).

## Deployment

### Local

To run the API server directly,
make sure you have [test credentials](#test-credentials),
then:

1. (Re)build and load the latest server image into the local Docker daemon.
   ```bash
   bazel run //api/v1:load-image
   ```
2. Start a local API server using your test credentials
   and listening on some TCP port of `localhost` *e.g.* 61803.
   ```bash
   docker run -e TLS_KEY="$(< dev/tls/localhost.key)" -e TLS_CERT="$(< dev/tls/localhost.cert)" -p 61803:443 --rm vimana-api-v1:latest
   ```
3. Invoke RPCs *e.g.* using [`grpcurl`](https://github.com/fullstorydev/grpcurl)
   (if you skipped [importing the root CA](#test-credentials),
   you must also pass the `-insecure` option to skip validation).
   ```bash
   grpcurl localhost:61803 vimana.api.v1.Domains/Create
   ```

### Minikube

To run the API server in a local cluster environment,
make sure you have [test credentials](#test-credentials),
then:

1. Set up a local Docker [registry](https://hub.docker.com/_/registry).
   With `--restart=always`,
   the Docker daemon will automatically restart on exit or reboot,
   and ensure that the registry container is always running,
   so this only needs to happen once.
   ```bash
   docker run -d -p 5000:5000 --restart=always --name=registry registry:latest
   ```
2. (Re)build and push the latest server image to our local registry.
   ```bash
   bazel run //api/v1:push-image-local
   ```
3. Minikube sets up the special hostname
   [`host.minikube.internal`](https://minikube.sigs.k8s.io/docs/handbook/host-access/)
   to access the local host (from our perspective).
   Since our local registry uses cleartext HTTP,
   we have to start Minikube with `--insecure-registry`.
   ```bash
   minikube start --insecure-registry='host.minikube.internal:5000'
   ```