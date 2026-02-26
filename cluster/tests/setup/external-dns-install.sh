# Install the ExternalDNS Helm chart in the current cluster.
# https://kubernetes-sigs.github.io/external-dns/latest/

# Configure ExternalDNS to watch for DNSEndpoint CRDs
# and write SkyDNS-formatted entries to etcd.
# Silence some noisy warnings about normal symlinks in the Bazel sandbox.
"$helm" upgrade --install \
  external-dns "$external_dns_helm_chart" \
  --namespace='kube-system' \
  --set=provider.name='coredns' \
  --set=sources='{crd}' \
  --set=extraArgs.coredns-prefix='/skydns/' \
  --set=env[0].name=ETCD_URLS \
  --set='env[0].value=http://etcd-server.cert-manager.svc.cluster.local:2379' \
  --set=rbac.create=true \
  2> >(sed '/found symbolic link in path\. Contents of linked file included and used/d' >&2)
