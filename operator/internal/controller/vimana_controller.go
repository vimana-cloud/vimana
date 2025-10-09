package controller

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	gwapi "sigs.k8s.io/gateway-api/apis/v1"
	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

// Definitions to manage status conditions
const (
	gatewayClassName = "envoy-gateway"

	// conditionTypeAvailable represents the steady-state existing status of a Vimana.
	conditionTypeAvailable = "Available"
)

// VimanaReconciler reconciles a Vimana object.
type VimanaReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

// +kubebuilder:rbac:groups=api.vimana.host,resources=vimanas,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=api.vimana.host,resources=vimanas/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=api.vimana.host,resources=vimanas/finalizers,verbs=update

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
// TODO(user): Modify the Reconcile function to compare the state specified by
// the Vimana object against the actual cluster state, and then
// perform operations to make the cluster state reflect the state specified by
// the user.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.19.0/pkg/reconcile
func (r *VimanaReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	// TODO: Somehow, it should be impossible for multiple to co-exist in a namespace.

	vimana := &apiv1alpha1.Vimana{}
	err := r.Get(ctx, req.NamespacedName, vimana)
	if err != nil {
		if apierrors.IsNotFound(err) {
			log.Info("Vimana not found, assumed deleted", "namespace", req.Namespace, "name", req.Name)
			return ctrl.Result{}, nil
		}
		// Error reading the object; re-enqueue the request.
		log.Error(err, "Failed to get Vimana", "namespace", req.Namespace, "name", req.Name)
		return ctrl.Result{}, err
	}

	// Set the status as Unknown when no status is available.
	if len(vimana.Status.Conditions) == 0 {
		meta.SetStatusCondition(
			&vimana.Status.Conditions,
			metav1.Condition{
				Type:    conditionTypeAvailable,
				Status:  metav1.ConditionUnknown,
				Reason:  "Reconciling",
				Message: "Starting reconciliation",
			},
		)
		if err = r.Status().Update(ctx, vimana); err != nil {
			log.Error(err, "Failed to initialize Vimana status", "namespace", req.Namespace, "name", req.Name)
			return ctrl.Result{}, err
		}

		// Re-fetch the CR after updating the status.
		// It will almost certainly just hit the cache,
		// but this can help avoid errors that say
		// "the object has been modified, please apply your changes to the latest version and try again".
		if err := r.Get(ctx, req.NamespacedName, vimana); err != nil {
			log.Error(err, "Failed to re-get Vimana", "namespace", req.Namespace, "name", req.Name)
			return ctrl.Result{}, err
		}
	}

	// List all the domains in the namespace.
	domains := &apiv1alpha1.DomainList{}
	err = r.List(ctx, domains, client.InNamespace(req.Namespace))
	if err != nil {
		log.Error(err, "Failed to list Domains", "namespace", req.Namespace)
		return ctrl.Result{}, err
	}
	if len(domains.Items) == 0 {
		// A gateway requires at least 1 listener to be valid,
		// If there are no domains, there are no listeners, and there can be no gateway.
		// Make sure it does not exist.
		gatewayName := vimana.Name + ".gateway"
		actualGateway := &gwapi.Gateway{}
		err = r.Get(ctx, types.NamespacedName{Name: gatewayName, Namespace: vimana.Namespace}, actualGateway)
		if err != nil {
			if !apierrors.IsNotFound(err) {
				// The gateway does exist, but there was some other error.
				log.Error(err, "Failed to look up the existing Gateway", "namespace", vimana.Namespace, "name", gatewayName)
				return ctrl.Result{}, err
			}
			// The gateway already does not exist. Continue.
		} else {
			// The gateway exists. Delete it.
			if err = r.Delete(ctx, actualGateway); err != nil {
				log.Error(err, "Failed to delete the existing Gateway", "namespace", vimana.Namespace, "name", gatewayName)
				return ctrl.Result{}, err
			}
		}
		return ctrl.Result{}, nil

	} else {
		// Construct the Gateway spec.
		// These values are the same for every listener.
		allowedRoutes := &gwapi.AllowedRoutes{
			Kinds: []gwapi.RouteGroupKind{
				{Kind: gwapi.Kind("GRPCRoute")},
			},
		}
		secretKind := (*gwapi.Kind)(ptr.To("Secret"))

		var listeners []gwapi.Listener
		for _, domain := range domains.Items {
			canonical := fmt.Sprintf("%s.app.vimana.host", domain.Spec.Id)
			namespace := (*gwapi.Namespace)(ptr.To(domain.GetNamespace()))
			listeners = append(listeners, listener(canonical, namespace, allowedRoutes, secretKind))
			for _, alias := range domain.Spec.Aliases {
				listeners = append(listeners, listener(alias, namespace, allowedRoutes, secretKind))
			}
		}

		expectedGateway := &gwapi.Gateway{
			ObjectMeta: metav1.ObjectMeta{
				Name:      vimana.Name + ".gateway",
				Namespace: vimana.Namespace,
			},
			Spec: gwapi.GatewaySpec{
				GatewayClassName: gatewayClassName,
				Listeners:        listeners,
			},
		}

		// Set the Vimana as the owner of the Gateway.
		if err := ctrl.SetControllerReference(vimana, expectedGateway, r.Scheme); err != nil {
			log.Error(err, "Failed to set owner reference for Gateway", "namespace", expectedGateway.Namespace, "name", expectedGateway.Name)
			return ctrl.Result{}, err
		}

		// Create or Update the existing Gateway.
		actualGateway := &gwapi.Gateway{}
		err = r.Get(ctx, types.NamespacedName{Name: expectedGateway.Name, Namespace: expectedGateway.Namespace}, actualGateway)
		if err != nil && apierrors.IsNotFound(err) {
			// The gateway does not exist. Create it.
			log.Info("Creating a new Gateway", "namespace", expectedGateway.Namespace, "name", expectedGateway.Name)
			err = r.Create(ctx, expectedGateway)
			if err != nil {
				log.Error(err, "Failed to create the new Gateway", "namespace", expectedGateway.Namespace, "name", expectedGateway.Name)
				return ctrl.Result{}, err
			}
		} else if err != nil {
			log.Error(err, "Failed to get the existing Gateway", "namespace", expectedGateway.Namespace, "name", expectedGateway.Name)
			return ctrl.Result{}, err
		} else {
			// Update the existing Gateway by overwriting it with the expected spec.
			actualGateway.Spec = expectedGateway.Spec
			log.Info("Updating existing Gateway", "namespace", actualGateway.Namespace, "name", actualGateway.Name)
			err = r.Update(ctx, actualGateway)
			if err != nil {
				log.Error(err, "Failed to update Gateway", "namespace", actualGateway.Namespace, "name", actualGateway.Name)
				return ctrl.Result{}, err
			}
		}

		// TODO: Update conditions, etc.

		return ctrl.Result{}, nil
	}
}

// Return the Gateway Listener object for the given domain name in the given namespace.
// This will have a specific name that looks like `l-<hash>` with the hex-encoded SHA-256 hash of the domain name;
// guaranteed valid and *probably* unique per domain.
// The associated certificate is expected to have the name `c-<hash>` for the same reasons.
func listener(domain string, namespace *gwapi.Namespace, allowedRoutes *gwapi.AllowedRoutes, secretKind *gwapi.Kind) gwapi.Listener {
	hash := sha256.Sum256([]byte(domain))
	hashHex := hex.EncodeToString(hash[:])
	return gwapi.Listener{
		Name:     gwapi.SectionName(fmt.Sprintf("l-%s", hashHex)),
		Protocol: gwapi.HTTPSProtocolType,
		Port:     443,
		Hostname: (*gwapi.Hostname)(ptr.To(domain)),
		TLS: &gwapi.ListenerTLSConfig{
			CertificateRefs: []gwapi.SecretObjectReference{
				{
					Kind:      secretKind,
					Name:      gwapi.ObjectName(fmt.Sprintf("c-%s", hashHex)),
					Namespace: namespace,
				},
			},
		},
		AllowedRoutes: allowedRoutes,
	}
}

// SetupWithManager sets up the controller with the Manager.
func (r *VimanaReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&apiv1alpha1.Vimana{}).
		Watches(&apiv1alpha1.Domain{}, handler.EnqueueRequestsFromMapFunc(r.domainReconciliationRequest)).
		Owns(&gwapi.Gateway{}).
		Complete(r)
}

func (r *VimanaReconciler) domainReconciliationRequest(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	// If there is an existing Vimana in this namespace, reconcile it.
	namespace := obj.(*apiv1alpha1.Domain).GetNamespace()
	vimana, err := r.getSingleVimana(ctx, namespace)
	if err != nil {
		log.Error(err, "Failed getting Vimana for domain", "namespace", namespace)
	}

	return []reconcile.Request{
		{NamespacedName: types.NamespacedName{
			Name:      vimana.Name,
			Namespace: vimana.Namespace,
		}},
	}
}

// If there is a single Vimana resource in the given namespace, return it.
// Otherwise, return an error.
func (r *VimanaReconciler) getSingleVimana(ctx context.Context, namespace string) (*apiv1alpha1.Vimana, error) {
	vimanas := &apiv1alpha1.VimanaList{}
	err := r.List(ctx, vimanas, client.InNamespace(namespace))
	if err != nil {
		return nil, err
	}
	vimanasCount := len(vimanas.Items)
	if vimanasCount > 1 {
		return nil, fmt.Errorf("There are %d existing Vimanas in this namespace", vimanasCount)
	}
	if vimanasCount == 0 {
		return nil, fmt.Errorf("There are no existing Vimanas in this namespace")
	}
	return &vimanas.Items[0], nil
}
