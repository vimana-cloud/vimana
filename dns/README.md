# DNS and External Routing

## Tenant Domains

Every tenant service is assigned a
[V4 UUID](https://en.wikipedia.org/wiki/Universally_unique_identifier#Version_4_(random))
as its *service ID*,
and given a *canonical domain* of the form `<service-id>.app.vimana.host`.
In order to resolve these names,
Vimana runs a dedicated DNS server
at `dns.vimana.host` (VDNS):

```
app.vimana.host. 3600 IN NS dns.vimana.host.
```

When a service is first created,
VDNS is configured to resolve address queries (`A`/`AAAA`)
for the canonical domain,
pointing to ingress nodes according to the service's deployment configuration
and/or the client's IP address.

Since Vimana only serves traffic over HTTPS,
A TLS certificate for the canonical domain must be provisioned
before the service can actually be used.

### Certificate Provision

Upon receiving the initial address configuration for a new service,
VDNS also initiates the certificate provisioning process:

1. VDNS creates an [ACME order](https://www.rfc-editor.org/rfc/rfc8555.html)
   for the domain and initiates an `http-01` challenge.
2. The CA sends a `GET` request
   to `http://<canonical-domain>/.well-known/acme-challenge/<token>`,
   which is routed to some ingress node.
   All ingress nodes have access to
   Vimana's ACME account key [thumbprint](https://www.rfc-editor.org/rfc/rfc8555.html#section-8.1),
   enabling any one to directly respond to any challenge
   by copying the token from the request
   (taking care to validate that the token is base64url-encoded,
   to prevent XSS attacks).
3. VDNS polls the `finalize` endpoint with exponential backoff
   until the status is `valid`.
4. VDNS downloads the certificate
   and creates an `HTTPS` record with Encrypted Client Hello:
   ```
   <domain> 3600 IN HTTPS 1 . ( alpn=h2,h3 ech=<ech-config> )
   ```
   This enables subsequent requests to skip HTTP/1 and the TLS handshake. 
5. VDNS also adds the certificate to the service's configuration
   which is broadcast to all ingress nodes,
   so subsequent HTTPS traffic can be served.

### Aliases

Users are encouraged to set up alias domains
using either an `HTTPS` or `CNAME` record, for example:

```
api.example.com. 3600 IN HTTPS 0 <service-id>.app.vimana.host.
```

In order to provision a certificate for an alias domain,
an agent must explicitly add it:

```bash
vimana service add-domain "<alias-domain>"
```

The Vimana API server looks for
an alias-mode `HTTPS`, `SVCB`, or `CNAME` record at the alias domain.
If it finds one that points to a Vimana app,
it kicks off the same [certificate provision](#certificate-provision) outlined above,
except using the alias domain instead of the canonical domain.

After this process is complete,
VDNS might have 3 records for our example service:

```
<canonical-domain> 3600 IN HTTPS 1 .                  ( alpn=h2,h3 ech=<ech-config> )
<alias-domain>     3600 IN HTTPS 1 <canonical-domain> ( alpn=h2,h3 ech=<ech-config> )
<canonical-domain>  300 IN A       <ingress-ip>
```