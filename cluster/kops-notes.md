# Debugging  kOps

The `Cluster` resource is documented [here](https://kops.sigs.k8s.io/cluster_spec/).

## Node Initialization

kOps uses [cloud-init] to initialize each node.
This is configued using a Bash script
which is attached to each VM as a piece of metadata called `user-data`.
For kOps, that script downloads and executes an official binary release of [`nodeup`],
which does all the initialization legwork.

[cloud-init]: https://cloud-init.io/
[`nodeup`]: https://kops.sigs.k8s.io/operations/troubleshoot/#nodeup
