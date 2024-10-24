# Walkthrough

This narrative walkthrough illustrates how Vimana works,
both from an internal and external perspective.
For an encyclopedic overview,
see the [internal overview](internal-overview.md).

## Signing In

Vimana delegates authentication to OIDC providers
like Google and GitHub.
Get an ID token with:

```bash
vimana user login
```

Choose your ID provider on the login page &mdash;
e.g. Google &mdash;
and the CLI eventually ends up getting a signed
[ID token](https://openid.net/specs/openid-connect-core-1_0.html#IDToken)
who's JWT payload looks like this:

```json
{
  "iss": "https://accounts.google.com",
  "azp": "32555940559.apps.googleusercontent.com",
  "aud": "32555940559.apps.googleusercontent.com",
  "sub": "106778792747893319492",
  "email": "user@example.com",
  "email_verified": true,
  "at_hash": "6iURwmFg8OolEu7-6yrG6w",
  "iat": 1729713046,
  "exp": 1729716646,
}
```

This token is cached locally
and sent in the `Authorization` request header
for subsequent API calls.

Vimana maintains
it looks up the 

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