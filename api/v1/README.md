# Viman API V1

All user interactions with the platform happen through the API.

## Manual Testing

Run a local dev server with the following commands:

1. (Re)build and load the latest server image into the local Docker daemon.
   ```bash
   bazel run //api/v1:load-image
   ```
2. Generate a new RSA private key and self-signed TLS certificate for `localhost`
   and save the environment variables to `dev/self-signed.env`.
   This only needs to happen once, but regeneration is perfectly fine.
   ```bash
   bazel run //dev:generate-tls-credentials
   ```
3. Load the previously-generated credentials
   and start a local API server listening on TCP port 61803 of `localhost`.
   ```bash
   source dev/self-signed.env
   docker run -e TLS_KEY -e TLS_CERT -p 127.0.0.1:61803:443/tcp --rm vimana-api-v1:latest
   ```
4. Invoke RPCs with [`grpcurl`](https://github.com/fullstorydev/grpcurl),
   using `-insecure` to skip validating the self-signed certificate.
   ```bash
   grpcurl -insecure localhost:61803 vimana.api.v1.Domains/Create
   ```