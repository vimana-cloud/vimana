# Walkthrough

This narrative
(as opposed to [encyclopedic](internal-overview.md))
walkthrough illustrates how Vimana works from an internal perspective.

## Sign In

Vimana delegates authentication to OIDC providers
like Google or Vimana itself.
This means you don't have to create a new account to get started.
Simply get an ID token with:

```bash
vimana user login
```

Choose your ID provider on the login page &mdash;
*e.g.* Google &mdash;
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

Each user is uniquely identified by their issuer (`iss`) and subject (`sub`),
but a verified email is also required, which is used as their display name.
Verifying email also helps prevent abuse.

## Create a Domain

Services exist within a canonical domain

## Create a Service

By default, services are global and optimized for cost.
That means requests from anywhere will be routed to the cluster
with the current lowest running costs globally.
To create such a service, simply run:

```bash
vimana service create
```

On success, the new service ID is returned.

Services can also be configured during creation
by providing a [deployment configuration](TODO)
in [Protobuf text format](https://protobuf.dev/reference/protobuf/textformat-spec/):

```bash
vimana service create << END
  # A regional deployment restricts traffic to the region in which it originates.
  regional: {

    # Variables can be used for dynamic configuration.
    # Run in any datacenter in the Eastern US
    # or any AWS datacenter in Western Europe.
    regions: ["/us-east", "aws/eu-west"]

    # Enabling failover allows traffic to be routed across regions
    # in case of unavailability in the client's region.
    failover: true
  }

  # Route requests to minimize latency, rather than cost,
  # within the constraints of the regional deployment.
  target: LATENCY
END
```

## Deploy a Component

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

    // Components must include an explicit service ID.
    id: "d7bd258f-46ff-4fb4-a02d-efa4d096f810"

    // Components must be versioned.
    version: "1.0.0"

    // This version exports telemetry to Grafana.
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

## K8s API Mapping

```yaml
apiVersion: v1
kind: Service
metadata:
  name: 00000000-0000-0000-0000-000000000000
spec:
  type: ClusterIP
  selector:
    service: 00000000-0000-0000-0000-000000000000
  ports:
    - protocol: TCP
      port: 8080
      targetPort: 80
```
