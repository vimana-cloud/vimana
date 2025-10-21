package controller

import (
	"context"
	"reflect"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
	gwapi "sigs.k8s.io/gateway-api/apis/v1"

	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

var (
	// Turn this into a variable so we can take its address.
	grpcPortNumberForGateway = gwapi.PortNumber(grpcPortNumber)

	// K8s resource kind for a Service.
	serviceKind = gwapi.Kind("Service")

	// Make this a variable so that it has an address and we can get a pointer to it.
	exactMethodMatch = gwapi.GRPCMethodMatchExact
)

// DomainReconciler reconciles a Domain object
type DomainReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

// Return true iff the two objects are *not* equal.
func grpcRouteSpecDiffers(actual, expected *gwapi.GRPCRoute) bool {
	return !reflect.DeepEqual(actual.Spec, expected.Spec)
}

// Mutate the "spec" value of the receiver to match that of the other object.
func grpcRouteCopySpec(receiver, giver *gwapi.GRPCRoute) {
	receiver.Spec = giver.Spec
}

// +kubebuilder:rbac:groups=api.vimana.host,resources=domains,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=api.vimana.host,resources=domains/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=api.vimana.host,resources=domains/finalizers,verbs=update

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.19.0/pkg/reconcile
func (r *DomainReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	domain := &apiv1alpha1.Domain{}
	err := r.Get(ctx, req.NamespacedName, domain)
	if err != nil {
		if apierrors.IsNotFound(err) {
			logger.Info("Domain not found, assumed deleted", "namespace", req.Namespace, "name", req.Name)
			return ctrl.Result{}, nil
		}
		// Error reading the object; re-enqueue the request.
		logger.Error(err, "Failed to get Domain", "namespace", req.Namespace, "name", req.Name)
		return ctrl.Result{}, err
	}

	// List all the servers under the domain.
	servers := &apiv1alpha1.ServerList{}
	err = r.List(ctx, servers, client.InNamespace(req.Namespace), client.MatchingLabels{labelDomainKey: domain.Spec.Id})
	if err != nil {
		logger.Error(err, "Failed to list Servers", "namespace", req.Namespace, "domain", domain.Spec.Id)
		return ctrl.Result{}, err
	}

	hostnames := make([]gwapi.Hostname, 0, len(domain.Spec.Aliases)+1)
	hostnames = append(hostnames, gwapi.Hostname(canonicalDomain(domain.Spec.Id)))
	for _, alias := range domain.Spec.Aliases {
		hostnames = append(hostnames, gwapi.Hostname(alias))
	}

	rules := make([]gwapi.GRPCRouteRule, 0, len(servers.Items))
	for _, server := range servers.Items {
		matches := make([]gwapi.GRPCRouteMatch, 0, len(server.Spec.Services))
		for _, service := range server.Spec.Services {
			matches = append(matches, gwapi.GRPCRouteMatch{
				Method: &gwapi.GRPCMethodMatch{
					Type:    &exactMethodMatch,
					Service: &service,
				},
			})
		}

		backendRefs := make([]gwapi.GRPCBackendRef, 0, len(server.Spec.VersionWeights))
		for version, weight := range server.Spec.VersionWeights {
			backendRefs = append(backendRefs, gwapi.GRPCBackendRef{
				BackendRef: gwapi.BackendRef{
					BackendObjectReference: gwapi.BackendObjectReference{
						Name: gwapi.ObjectName(prefixed(hashed(componentName(domain.Spec.Id, server.Spec.Id, version)), 's')),
						Kind: &serviceKind,
						Port: &grpcPortNumberForGateway,
					},
					Weight: &weight,
				},
			})
		}

		rules = append(rules, gwapi.GRPCRouteRule{
			Matches:     matches,
			BackendRefs: backendRefs,
		})
	}

	grpcRouteNamespacedName := types.NamespacedName{
		Name:      domain.Spec.Id,
		Namespace: req.Namespace,
	}
	expectedGrpcRoute := &gwapi.GRPCRoute{
		ObjectMeta: metav1.ObjectMeta{
			Name:      grpcRouteNamespacedName.Name,
			Namespace: grpcRouteNamespacedName.Namespace,
			Labels: map[string]string{
				labelDomainKey: domain.Spec.Id,
			},
		},
		Spec: gwapi.GRPCRouteSpec{
			CommonRouteSpec: gwapi.CommonRouteSpec{
				ParentRefs: []gwapi.ParentReference{
					{
						Name: gwapi.ObjectName(gatewayName(domain.Spec.Vimana)),
						// The default namespace for the referent is the same as that of the referrer.
					},
				},
			},
			Hostnames: hostnames,
			Rules:     rules,
		},
	}

	// Set the Domain as the owner of the GRPCRoute.
	if err = ctrl.SetControllerReference(domain, expectedGrpcRoute, r.Scheme); err != nil {
		logger.Error(err, "Failed to set owner reference for GRPCRoute", "namespace", expectedGrpcRoute.Namespace, "name", expectedGrpcRoute.Name)
		return ctrl.Result{}, err
	}

	// Create or Update the GRPCRoute.
	err = ensureResourceHasSpecAndLabels(r.Client, ctx, grpcRouteNamespacedName, &gwapi.GRPCRoute{}, expectedGrpcRoute, grpcRouteSpecDiffers, grpcRouteCopySpec)
	if err != nil {
		return ctrl.Result{}, err
	}

	return ctrl.Result{}, nil
}

// SetupWithManager sets up the controller with the Manager.
func (r *DomainReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&apiv1alpha1.Domain{}).
		Watches(&apiv1alpha1.Server{}, handler.EnqueueRequestsFromMapFunc(r.serverReconciliationRequest)).
		Owns(&gwapi.GRPCRoute{}).
		Complete(r)
}

func (r *DomainReconciler) serverReconciliationRequest(ctx context.Context, obj client.Object) []reconcile.Request {
	logger := log.FromContext(ctx)
	server := obj.(*apiv1alpha1.Server)

	domainId := server.Labels[labelDomainKey]
	if domainId == "" {
		// The server resource has no domain label (an invariant has been violated).
		// Hopefully this never happens.
		logger.Error(nil, "Server lacks a domain label", "namespace", server.Namespace, "name", server.Name)
		return nil
	}

	// We could just enqueue the request now,
	// but if the domain does not exist,
	// the reconciliation function would consider it a normal "domain deleted" event,
	// rather than the erroneous state where a server outlives its domain,
	// which is what it actually is.
	domainNamespacedName := types.NamespacedName{
		Name:      domainId, // TODO: Is this always a valid K8s resource name?
		Namespace: server.Namespace,
	}
	domain := &apiv1alpha1.Domain{}
	err := r.Get(ctx, domainNamespacedName, domain)
	if err != nil {
		logger.Error(err, "Failed getting Domain for Server", "namespace", server.Namespace, "name", server.Name)
		return nil
	}

	return []reconcile.Request{{NamespacedName: domainNamespacedName}}
}
