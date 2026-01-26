# Cluster Management

Vimana aims to make provisioning clusters on various cloud providers as easy as possible,
but currently, only GCP is supported.

## Profiles

Profiles provide a convenient way
to keep track of the private details related to cluster management.

If you haven't yet, edit [`cluster/profile/profiles.yaml`],
replacing one of the example profile names (*i.e.* `gcp-example.com`)
with a new name (*e.g.* `my-cluster.net`).
It *does not* have to be a real domain.

Edit the following fields:

- `state-store` should identify a usable [kOps state store].
  This can be the URI of a Google Storage bucket that you own.
- `runtime-uri` is the URI of an OCI image (repository and tag)
  that will be used to fetch a build of the Vimana runtime into the cluster.
- `operator-uri` is the URI of an OCI image (repository and tag)
  that will be used to fetch the Vimana operator image into the cluster.
- For GCP profiles, `project` is the ID of the GCP project that will own the cluster.

## GCP

To use the GCP backend,
first ensure you have [application default credentials] available on your machine.
The simplest way to do this for a normal Google account is to run:

```bash
gcloud auth application-default login
```

[application default credentials]: https://docs.cloud.google.com/docs/authentication/application-default-credentials

### Cluster Provision

To push fresh runtime or operator artifact(s)
to the repositories you configured in the profile
(`runtime-uri` and `operator-uri`, respectively),
run the following commands,
replacing the value in angle brackets with the same value you configued in the profile:

```bash
bazel run //runtime:artifact-push -- --repository=<RUNTIME-URI>
bazel run //operator:image-push -- --repository=<OPERATOR-URI>
```

Each command labels the freshly-pushed artifact with the `latest` tag.
You can set additional tags by passing `--tag=<TAG>` options.

Once the profile has been configured,
and fresh artifacts have been pushed,
create your cluster:

```bash
bazel run //cluster:create --//cluster/profile='gcp-example.com' # or whatever you named it
```

You can interact with the new cluster using `kubectl`.

When you're done with it, shut it down:

```bash
bazel run //cluster:destroy --//cluster/profile='gcp-example.com'
```

[`cluster/profile/profiles.yaml`]: /cluster/profile/profiles.yaml
[kOps state store]: https://kops.sigs.k8s.io/state/

## Testing

The [end-to-end tests], which run against a local Kind cluster by default,
can also be configured to run against a live cluster in the cloud.
Currently, this is the only way to run the end-to-end tests on a Mac.

The E2E tests will run against the the current cluster context
as identified by `kubectl config current-context`.
By default, that's the most recently created cluster,
be it local or cloud-based.
So, if you just ran `//cluster:create`,
you're already set up to test against that cluster.
A different cluster can be selected with `kubectl config use-context`.

The main difference between testing locally and testing in the cloud
is that a cloud cluster will not have access to your local container registry
for pushing and pulling Wasm component images,
so you must specify one:

```bash
bazel test //cluster/tests:mvp-test --//cluster/tests:registry=ghcr.io/user/repo
```

This registry will be used for distributing Vimana components
before and during test execution.
Components pushed to the registry
are *not* automatically cleaned up once the test is over.

[end-to-end tests]: /cluster/tests/
