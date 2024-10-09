# Overview

Vimana is organized into isolated zones,
each corresponding to a single Kubernetes cluster.
For example, `aws/us-east-1-bos-1a`
would run exclusively in AWS' `us-east-1-bos-1a` zone,
while `gcp/us-east1-a`
would run exclusively in GCP's `us-east1-a` zone.

W

Zones are grouped into regions.
For instance, `all/us-east`
may include clusters in any of AWS, GCP, or Azure's zones in the eastern US.
Most regions are multi-cloud.

Domains organize services into groups.
Each user account can belong to zero or more domains.
These groups are meant to be small.

A customer could deploy to a region,
but may deploy to a specific cluster,
such as to optimize latency to a database in a known location.

## Zones

Each cluster (zone) comprises 4 components:

- **Ingress** nodes receive requests from external clients.
  They communicate with the *control* nodes
  to route traffic to the *work* nodes.
- **Control** nodes serve the control plane;
  the K8s API, and other zone administration.
- **Work** nodes serve hosted services to external clients
  (via *ingress*).
- **DNS** servers manage the

## Regions

Each region assigns traffic to its various clusters
according to a bias.
The default bias is compute cost.
`eu.multi.vimana.host` would have bias against compute cost,
while `prox.eu.multi.vimana.host` would have proximity bias,
and `mem.eu.multi.vimana.host` would have bias against memory cost,
all in the EU multi-cloud region.

Regions are configured entirely via [DNS](#dns).

## DNS

A centralized DNS system underpins the whol

# Deployment Walk-Through

Understand how services in Vimana are deployed
with a detailed walk-through.

## Initial Deployment

Alice wants to deploy the following service for the first time:

```proto
// api.proto

syntax = "proto3";

// Domain: "example.com"
package com.example;

service HelloWorld {
  rpc SayHello(HelloRequest) returns (HelloResponse) {}
}

message HelloRequest {
  string name = 1;
}

message HelloResponse {
  bool talk_to_me = 1;
}
```

Alice generates the bindings,
implements version `1.0`,
and pushes the implementation.

Now, it's time to deploy:

```bash
vimana deploy com.example.HelloWorld@1.0 100%
```

This invokes [`Domains/Deploy`][TODO], which does:

1. Lock the key `example.com:com.example.HelloWorld` in the service config store.
   Concurrent calls to `Domains/Deploy` for a given service are disallowed.
2. Estimate how many workers `N` will be needed for version `1.0`
   based on the number of workers serving all prior deployed versions
   and the percentage of traffic being stolen by `1.0`.
   The minimum value of `N` is one.
   Since `1.0` is the first ever version of this particular service,
   `N` would be one.
3. Pick `N` good candidates to serve `1.0`
   based on available memory and bandwidth,
   spinning up new nodes if necessary.
   Send each candidate [`Control/Preload`][TODO]
   so they'll be ready to serve traffic sooner.
4. Update the service config for `example.com:com.example.HelloWorld`,
   including the new candidates, and release the lock.
   The service config includes information like:
   - How to pick a version to serve a request.
   - Which work node(s) can serve each version.

# Ingress

Ingress receives a request.
It checks the following places, in order, for the domain:

1. The [`Host` header], if present.
2. The [`:authority` header] for HTTP/2 and /3,
   or the [request target] for HTTP/1.

[`Host` header]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/host
[`:authority` header]: https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md#protocol
[request target]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Messages#request_line

It gets the service name from the HTTP path,
and loads the configuration for that domain/service from the service config store,
caching locally.
The request is forwarded to a good work node one
for the version chosen for the request,
with the chosen version added as the request header `TODO`.

# Work

A work node receives a request,
and extracts the domain, service name, and version,
which are used to look up the implementation.
[TODO]: #todo
