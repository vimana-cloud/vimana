# Walkthrough

This walkthrough illustrates how Vimana works,
both from a UX and behind-the-scenes perspective.

## Signing In

Vimana delegates authentication to OIDC providers
like Google and GitHub.
Get an ID token with:

```bash
vimana user login
```

The CLI first tries to bind to port
[61803](https://en.wikipedia.org/wiki/Golden_ratio).
If successful,
it then opens `vimana.host/login?cli=auto` in a browser.
That's because Vimana's CLI-specific OAuth apps
redirect to `http://127.0.0.1:61803` for the OIDC callback.
If the CLI cannot bind to that port,
it instead opens `vimana.host/login?cli=manual`,
which re-uses Vimana's web-specific OAuth apps
to redirect to `https://api.vimana.host/callback/cli`,
which simply prints the ID token
so the user can manually copy and paste it into the CLI.

Either way, the user chooses their ID provider on the login page &mdash;
e.g. GitHub &mdash;
and the CLI eventually ends up getting a signed ID token that looks like this:

```json
{
  "iss": "https://github.com",
  "sub": "24400320",
  "aud": "Ov23lijpkaQ4ChTLTfAU",
  "exp": 1729586748,
  "iat": 1729583148,
  "nonce": "n-0S6_WzA2Mj",
}
```

This token is cached locally
and sent in the `Authorization` request header
for subsequent API calls.

## Domain Configuration 

Before deploying a service,
you need to claim a domain:

```bash
vimana domain claim example.com
```

### Interface and Implementation

Deploying a service starts with a service definition.
Here's an example proto file called `foo.proto`
for a [component](/docs/glossary.md#component)
that would be named `example.com:bar.FooService@1.0.0`:

```proto
syntax = "proto3";

package bar;

service FooService {

  // Define a simple unary RPC with JSON transcoding.
  rpc SayHello(HelloRequest) returns (HelloResponse) {

    // `vimana.http` is identical to `google.api.http`; you can use either.
    option (vimana.http) = {
      get: "/say-hi"
    };
  }

  // Define a bi-directional streaming RPC.
  // Non-unary RPC's do not support JSON transcoding.
  rpc PingPong(stream Ping) returns (stream Ping) {}

  // Configure service settings directly in the proto file, tracked by VCS.
  option (vimana.service) = {

    // Services must be versioned.
    version: "1.0.0"

    // Explicitly configure the domain.
    // If omitted, the domain would be inferred from the package.
    domain: "example.com"

    // By default, services are globally cost-optimized.
    // Use a more interesting deployment strategy for this example.
    deployment: {

      // A regional deployment restricts traffic to the region in which it originates.
      regional: {

        // Variables can be used for dynamic configuration.
        // Run in any datacenter in the Eastern US
        // or any AWS datacenter in Western Europe.
        regions: ["/us-east", "aws/eu-west"]

        // Enabling failover allows traffic to be routed across regions
        // in case of unavailability in the client's region.
        failover: true
      }

      // Route requests to minimize latency, rather than cost,
      // within the constraints of the regional deployment.
      target: LATENCY
    }

    // Export telemetry to Grafana.
    // Secret values should be stored as variables.
    otlp: {
      protocol: "http/protobuf"
      endpoint: "https://otlp-gateway-prod-eu-west-0.grafana.net/otlp"
      headers: "Authorization=Basic ${GRAFANA_TOKEN}"
    }
  };
}

message HelloRequest {
  string name = 1;
}

message HelloResponse {
  float enthusiasm = 1;
}

message Ping {}
```

The user generates their WIT file so they can implement the service.
This would look like the following, called `foo-1.0.0.wit`:

```wit
package bar@1.0.0;

world %foo-service {

  // Standard imports for the platform (context type, logging interface, etc.).
  import vimana:service/imports@1.0;

  export %say-hello: func(ctx: context, request: %hello-request) -> %hello-response;

  export %ping-pong: func(ctx: context, request: stream<%ping>) -> stream<%ping>;
}

record %hello-request {
  %name: string,
}

record %hello-response {
  %enthusiasm: f32,
}

// Empty records are disallowed in WIT.
// https://github.com/WebAssembly/component-model/pull/218
// This edge case is instead handled with a unitary enum.
enum %ping { %ping }
```

The user implements the `foo-service` world,
resulting in a compiled component.
Let's call it `foo-1.0.0.wasm`.
Now, it's time to upload the component
and matching Protobuf interface:

```bash
vimana push foo.proto foo-1.0.0.wasm
```

Vimana verifies that the interface matches the implementation,
but nothing is deployed yet.

### DNS Configuration

Every service on Vimana has a unique domain.
To find your service's domain, run:

```bash
vimana status example.com:bar.FooService
```

To make the service available at `example.com`,
configure the following
[`HTTPS` record](https://www.rfc-editor.org/rfc/rfc9460.html):

```
example.com. 3600 IN HTTPS 0 123456.app.vimana.host.
```

The record's `TargetName` (`123456.app.vimana.host.`)

### Initial Deployment

If `example.com:bar.FooService` has never been deployed before
(there are no pre-existing versions),
then deploying version `1.0.0` will cause it to serve 100% of traffic.

```bash
vimana deploy example.com:bar.FooService@1.0.0
```

Try exercising the unary action's HTTP