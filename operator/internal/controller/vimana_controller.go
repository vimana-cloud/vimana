package controller

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"reflect"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	envoygateway "github.com/envoyproxy/gateway/api/v1alpha1"
	nodev1 "k8s.io/api/node/v1"
	apiextensionsv1 "k8s.io/apiextensions-apiserver/pkg/apis/apiextensions/v1"
	gwapi "sigs.k8s.io/gateway-api/apis/v1"
	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

const (
	gatewayClassName  = "envoy-gateway"
	gatewayConfigName = "envoy-gateway-config"
	gatewayNamespace  = "envoy-gateway-system"
)

var (
	// expectedRuntimeClass is the expected state
	// of the globally shared RuntimeClass that identifies the Vimana runtime handler.
	expectedRuntimeClass = &nodev1.RuntimeClass{
		ObjectMeta: metav1.ObjectMeta{
			// RuntimeClass is cluster-scoped (no namespace).
			Name: runtimeClassName,
		},
		Handler: runtimeHandlerName,
		Scheduling: &nodev1.Scheduling{
			NodeSelector: map[string]string{
				"runtime": "vimanad",
			},
			Tolerations: []corev1.Toleration{
				{
					Key:      "runtime",
					Operator: corev1.TolerationOpEqual,
					Value:    "vimanad",
					Effect:   corev1.TaintEffectNoSchedule,
				},
			},
		},
	}
	// expectedGatewayClass is the expected state
	// of the globally shared GatewayClass used by all Vimana Gateways.
	// It's just Envoy Gateway.
	expectedGatewayClass = &gwapi.GatewayClass{
		ObjectMeta: metav1.ObjectMeta{
			// GatewayClass is cluster-scoped (no namespace).
			Name: gatewayClassName,
		},
		Spec: gwapi.GatewayClassSpec{
			ControllerName: "gateway.envoyproxy.io/gatewayclass-controller",
			// Points to a config resource that can be used to customize Envoy Gateway.
			ParametersRef: &gwapi.ParametersReference{
				Group:     "gateway.envoyproxy.io",
				Kind:      "EnvoyProxy",
				Name:      gatewayConfigName,
				Namespace: (*gwapi.Namespace)(ptr.To(gatewayNamespace)),
			},
			Description: ptr.To("Vimana Gateway class"),
		},
	}
)

// VimanaReconciler reconciles a Vimana object.
type VimanaReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

// Return true iff the two objects are *not* equal.
func envoyProxySpecDiffers(actual, expected *envoygateway.EnvoyProxy) bool {
	return !reflect.DeepEqual(actual.Spec, expected.Spec)
}

// Mutate the "spec" value of the receiver to match that of the other object.
func envoyProxyCopySpec(receiver, giver *envoygateway.EnvoyProxy) {
	receiver.Spec = giver.Spec
}

// Return true iff the two objects are *not* equal.
func gatewaySpecDiffers(actual, expected *gwapi.Gateway) bool {
	return !reflect.DeepEqual(actual.Spec, expected.Spec)
}

// Mutate the "spec" value of the receiver to match that of the other object.
func gatewayCopySpec(receiver, giver *gwapi.Gateway) {
	receiver.Spec = giver.Spec
}

// Return the expected state of the namespaced EnvoyProxy configuration
// for the Vimana with the given name.
func envoyProxyResource(name string) *envoygateway.EnvoyProxy {
	// Specify a static name for the gateway service by patching the Envoy proxy configuration.
	// https://github.com/envoyproxy/gateway/issues/2141
	serializedName, _ := json.Marshal(name) // Serializing a string should always succeed.
	var patchBuffer bytes.Buffer
	fmt.Fprintf(&patchBuffer, "{\"metadata\":{\"name\":%s}}", serializedName)

	// https://gateway.envoyproxy.io/docs/api/extension_types/#envoyproxy
	return &envoygateway.EnvoyProxy{
		ObjectMeta: metav1.ObjectMeta{
			Name:      gatewayConfigName,
			Namespace: gatewayNamespace,
		},
		Spec: envoygateway.EnvoyProxySpec{
			Provider: &envoygateway.EnvoyProxyProvider{
				Type: envoygateway.EnvoyProxyProviderTypeKubernetes,
				Kubernetes: &envoygateway.EnvoyProxyKubernetesProvider{
					EnvoyService: &envoygateway.KubernetesServiceSpec{
						Patch: &envoygateway.KubernetesPatchSpec{
							Value: apiextensionsv1.JSON{
								Raw: patchBuffer.Bytes(),
							},
						},
					},
				},
			},
		},
	}
}

// +kubebuilder:rbac:groups=api.vimana.host,resources=vimanas,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=api.vimana.host,resources=vimanas/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=api.vimana.host,resources=vimanas/finalizers,verbs=update
// +kubebuilder:rbac:groups=api.vimana.host,resources=domains,verbs=get;list;watch
// +kubebuilder:rbac:groups=node.k8s.io,resources=runtimeclasses,verbs=get;list;watch;create;update;patch
// +kubebuilder:rbac:groups=gateway.networking.k8s.io,resources=gatewayclasses,verbs=get;list;watch;create;update;patch
// +kubebuilder:rbac:groups=gateway.networking.k8s.io,resources=gateways,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=gateway.envoyproxy.io,resources=envoyproxies,verbs=get;list;watch;create;update;patch;delete

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.23.1/pkg/reconcile
func (r *VimanaReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	// TODO: Somehow, it should be impossible for multiple Vimanas to co-exist in a namespace.

	vimana := &apiv1alpha1.Vimana{}
	err := r.Get(ctx, req.NamespacedName, vimana)
	if err != nil {
		if apierrors.IsNotFound(err) {
			logger.Info("Vimana not found, assumed deleted", "namespace", req.Namespace, "name", req.Name)
			return ctrl.Result{}, nil
		}
		// Error reading the object; re-enqueue the request.
		logger.Error(err, "Failed to get Vimana", "namespace", req.Namespace, "name", req.Name)
		return ctrl.Result{}, err
	}

	// Set the status as Unknown when no status is available.
	if len(vimana.Status.Conditions) == 0 {
		err = updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionUnknown, "Reconciling", "Starting reconciliation")
		if err != nil {
			return ctrl.Result{}, err
		}
	}

	// Start by making sure that the Vimana runtime class exists.
	// This is a constant cluster-scoped resource that can be shared across namespaces.
	// Because of this potential for sharing, the Vimana resource is not added as an owner,
	// and the runtime class can outlive the original Vimana resource that caused it to be created.
	// It would have to be cleaned up manually if you ever wanted to get rid of it after creation.
	err = ensureClusterResourceExists(r.Client, ctx, runtimeClassName, &nodev1.RuntimeClass{}, expectedRuntimeClass)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionFalse, "RuntimeClass", "Failed to ensure RuntimeClass exists")
		return ctrl.Result{}, err
	}

	// We also have a gateway class that is cluster-scoped
	// and can similarly outlive it's creating Vimana.
	err = ensureClusterResourceExists(r.Client, ctx, gatewayClassName, &gwapi.GatewayClass{}, expectedGatewayClass)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionFalse, "GatewayClass", "Failed to ensure GatewayClass exists")
		return ctrl.Result{}, err
	}

	gatewayName := gatewayName(vimana.Name)
	gatewayNamespacedName := types.NamespacedName{Name: gatewayName, Namespace: vimana.Namespace}

	// Also make sure that the EnvoyProxy config exists.
	// This is namespace-scoped, but it always lives in the Gateway system namespace
	// (it does *not* inherit the namespace of the Vimana resource that owns it)
	// and has a name derived from the owner's name.
	expectedEnvoyProxy := envoyProxyResource(gatewayName)
	envoyProxyName := types.NamespacedName{Name: expectedEnvoyProxy.Name, Namespace: expectedEnvoyProxy.Namespace}

	// Set the Vimana as the owner of the EnvoyProxy.
	// TODO: This is illegal because they live in different namespaces. Revisit this!
	//if err = ctrl.SetControllerReference(vimana, expectedEnvoyProxy, r.Scheme); err != nil {
	//	logger.Error(err, "Failed to set owner reference for EnvoyProxy", "namespace", expectedEnvoyProxy.Namespace, "name", expectedEnvoyProxy.Name)
	//	updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionFalse, "EnvoyProxyOwner", "Failed to set owner for EnvoyProxy")
	//	return ctrl.Result{}, err
	//}

	err = ensureResourceHasSpecAndLabels(r.Client, ctx, envoyProxyName, &envoygateway.EnvoyProxy{}, expectedEnvoyProxy, envoyProxySpecDiffers, envoyProxyCopySpec)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionFalse, "EnvoyProxyUpsert", "Failed to update EnvoyProxy config")
		return ctrl.Result{}, err
	}

	// List all the domains in the namespace.
	domains := &apiv1alpha1.DomainList{}
	err = r.List(ctx, domains, client.InNamespace(req.Namespace))
	if err != nil {
		logger.Error(err, "Failed to list Domains", "namespace", req.Namespace)
		return ctrl.Result{}, err
	}

	if len(domains.Items) == 0 {
		// A gateway requires at least 1 listener to be valid,
		// If there are no domains, there are no listeners, and there can be no gateway.
		// Make sure it does not exist.
		err = ensureResourceDeleted(r.Client, ctx, gatewayNamespacedName, &gwapi.Gateway{})
		return ctrl.Result{}, err
	}

	// Construct the Gateway spec.
	// These values are the same for every listener.
	secretKind := (*gwapi.Kind)(ptr.To("Secret"))

	certificates := make([]gwapi.SecretObjectReference, 0, len(domains.Items))
	for _, domain := range domains.Items {
		namespace := (*gwapi.Namespace)(ptr.To(domain.GetNamespace()))
		for domainName := range domainNames(&domain, vimana) {
			certificates = append(certificates, gwapi.SecretObjectReference{
				Kind:      secretKind,
				Name:      gwapi.ObjectName(prefixed(hashed(domainName), 'c')),
				Namespace: namespace,
			})
		}
	}

	expectedGateway := &gwapi.Gateway{
		ObjectMeta: metav1.ObjectMeta{
			Name:        gatewayNamespacedName.Name,
			Namespace:   gatewayNamespacedName.Namespace,
			Annotations: issuerAnnotations(vimana),
		},
		Spec: gwapi.GatewaySpec{
			GatewayClassName: gatewayClassName,
			Listeners: []gwapi.Listener{
				// The HTTP listener enables ACME HTTP-01 challenge solving.
				// cert-manager creates temporary cleartext HTTPRoutes
				// that attach to this listener to serve challenge tokens.
				// When no HTTPRoutes are attached, clients receive 404 from Envoy.
				{
					Name:     "http-clear",
					Protocol: gwapi.HTTPProtocolType,
					Port:     80,
					AllowedRoutes: &gwapi.AllowedRoutes{
						Kinds: []gwapi.RouteGroupKind{
							{Kind: gwapi.Kind("HTTPRoute")},
						},
					},
				},
				// The HTTPS listener handles all data-plane traffic,
				// including gRPC and the gRPC-JSON transcoder.
				{
					Name:     "https",
					Protocol: gwapi.HTTPSProtocolType,
					Port:     443,
					AllowedRoutes: &gwapi.AllowedRoutes{
						Kinds: []gwapi.RouteGroupKind{
							{Kind: gwapi.Kind("GRPCRoute")},
							{Kind: gwapi.Kind("HTTPRoute")},
						},
					},
					TLS: &gwapi.ListenerTLSConfig{
						CertificateRefs: certificates,
					},
				},
			},
		},
	}

	// Set the Vimana as the owner of the Gateway.
	if err = ctrl.SetControllerReference(vimana, expectedGateway, r.Scheme); err != nil {
		logger.Error(err, "Failed to set owner reference for Gateway", "namespace", expectedGateway.Namespace, "name", expectedGateway.Name)
		updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionFalse, "GatewayOwner", "Failed to set owner for Gateway")
		return ctrl.Result{}, err
	}

	// Create or Update the Gateway.
	err = ensureResourceHasSpecAndLabels(r.Client, ctx, gatewayNamespacedName, &gwapi.Gateway{}, expectedGateway, gatewaySpecDiffers, gatewayCopySpec)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionFalse, "GatewayUpsert", "Failed to update Gateway for vimana")
		return ctrl.Result{}, err
	}

	err = updateAvailabilityStatus(r.Client, ctx, vimana, metav1.ConditionTrue, "Reconciled", "Successfully reconciled vimana")
	return ctrl.Result{}, err
}

// Return the cert-manager annotations for the Gateway based on the Vimana's issuer reference.
// Returns nil if no issuer is configured.
func issuerAnnotations(vimana *apiv1alpha1.Vimana) map[string]string {
	ref := vimana.Spec.IssuerRef
	if ref == nil {
		return nil
	}
	switch ref.Kind {
	case "ClusterIssuer":
		return map[string]string{"cert-manager.io/cluster-issuer": ref.Name}
	default:
		return map[string]string{"cert-manager.io/issuer": ref.Name}
	}
}

// SetupWithManager sets up the controller with the Manager.
func (r *VimanaReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&apiv1alpha1.Vimana{}).
		Watches(&apiv1alpha1.Domain{}, handler.EnqueueRequestsFromMapFunc(r.domainReconciliationRequest)).
		Owns(&gwapi.Gateway{}).
		Owns(&envoygateway.EnvoyProxy{}).
		Complete(r)
}

func (r *VimanaReconciler) domainReconciliationRequest(ctx context.Context, obj client.Object) []reconcile.Request {
	logger := log.FromContext(ctx)

	// If there is an existing Vimana in this namespace, reconcile it.
	namespace := obj.(*apiv1alpha1.Domain).GetNamespace()
	vimanas := &apiv1alpha1.VimanaList{}
	err := r.List(ctx, vimanas, client.InNamespace(namespace))
	if err != nil {
		logger.Error(err, "Failed getting Vimana for Domain", "namespace", namespace)
		return nil
	}
	vimanasCount := len(vimanas.Items)
	if vimanasCount > 1 {
		logger.Error(nil, fmt.Sprintf("There are %d existing Vimanas in this namespace, but there should be at most 1", vimanasCount), "namespace", namespace)
		return nil
	}
	if vimanasCount == 0 {
		logger.Error(nil, "There are no existing Vimanas in this namespace", "namespace", namespace)
		return nil
	}
	vimana := vimanas.Items[0]

	return []reconcile.Request{
		{NamespacedName: types.NamespacedName{
			Name:      vimana.Name,
			Namespace: vimana.Namespace,
		}},
	}
}
