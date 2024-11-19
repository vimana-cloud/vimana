# If minikube is already running, delete so we can start fresh.
minikube status &>/dev/null && minikube delete

# Start with:
# - The ability to load Docker containers from the host computer without TLS.
# - Enough resources to run Istio: https://istio.io/latest/docs/setup/platform-setup/minikube.
minikube start --insecure-registry 'host.minikube.internal:5000' --memory=16384 --cpus=4 && {

  # Start Istio in ambient mode (no sidecars).
  istioctl install --set profile=ambient --skip-confirmation && {

    # Set up the Getway API Custom Resource Definitions (CRDs):
    # https://github.com/kubernetes-sigs/gateway-api/releases.
    kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.0/standard-install.yaml
  }
}
