# Cluster Management

## Cloud Deployment

Vimana aims to make provisioning clusters on various cloud providers as easy as possible,
but currently, only GCP is supported.

To use the GCP backend,
first ensure you have [application default credentials] available on your machine.
The simplest way to do this for a normal Google account is to run:

```bash
gcloud auth application-default login
```

[application default credentials]: https://docs.cloud.google.com/docs/authentication/application-default-credentials

### Node Image

The first step is to build a node image
with the latest local build of the runtime.
Note that this is *not* a *Vimana* image,
but rather a regular VM [OS image]

If you own a project with ID `my-project-id`, you can run this:

```bash
bazel run //cluster/node:make-image -- --gcp-project="my-project-id"
```

That script will spin up a temporary GCE instance to build the node image,
then shut the instance down once the image is ready.
The whole process should take about five minutes
and cost less than $1.

[OS image]: https://docs.cloud.google.com/compute/docs/images

### Cluster

Profiles provide a convenient way
to keep track of the private details related to cluster management.

If you haven't yet, edit [`cluster/profiles/profiles.yaml`],
replacing `gcp-example-with-custom-node-image.com` with a new name,
*e.g.* `my-cluster.net`
(it *does not* have to be a real domain).
Edit the following fields:

- `state-store` should identify a usable [kOps state store].
  This can be the URI of a Google Storage bucket that you own.
- `project` is the ID of the project that will own the cluster.
  This may or may not be the same as `image-project`.
- `image-project` should be the same project you used to make the node image
  (`my-project-id` in the example above).
- `image-family` should be either `vimana` or `vimana-dirty`,
  depending on whether the node image was created from a clean Git worktree
  (the node image creation script will tell you which to use).
  The cluster will use the latest image within this family.

Once the profile is configured, use it to create your cluster:

```bash
bazel run //cluster:create -- 'my-cluster.net' # or whatever you named it
```

You can interact with the new cluster using `kubectl`.
Once you're done with it:

```bash
bazel run //cluster:destroy -- 'my-cluster.net'
```

[`cluster/profiles/profiles.yaml`]: cluster/profiles/profiles.yaml
[kOps state store]: https://kops.sigs.k8s.io/state/
