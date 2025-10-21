package controller

import (
	"context"
	"reflect"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

const (
	// The constant name of the single container that exists in each Vimana pod.
	grpcContainerName = "grpc"
)

var (
	// gRPC requires HTTP/2,
	// and traffic between the gateway and backends is cleartext.
	// TODO: We should always encrypt both at rest and in transit (thanks Snowden). Figure that out before GA.
	grpcAppProtocol = "kubernetes.io/h2c"

	// The pod spec requires the runtime class name to be expressed as a pointer.
	runtimeClassNamePtr = ptr.To(runtimeClassName)
)

// ComponentReconciler reconciles a Component object
type ComponentReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

// Return true iff the two objects are *not* equal.
func deploymentSpecDiffers(actual, expected *appsv1.Deployment) bool {
	// The number of replicas is controlled externally, probably by the HPA controller.
	// Make sure not to modify it in this controller.
	expected.Spec.Replicas = actual.Spec.Replicas
	return !reflect.DeepEqual(actual.Spec, expected.Spec)
}

// Mutate the "spec" value of the receiver to match that of the other object.
func deploymentCopySpec(receiver, giver *appsv1.Deployment) {
	receiver.Spec = giver.Spec
}

// Return true iff the two objects are *not* equal.
func serviceSpecDiffers(actual, expected *corev1.Service) bool {
	return !reflect.DeepEqual(actual.Spec, expected.Spec)
}

// Mutate the "spec" value of the receiver to match that of the other object.
func serviceCopySpec(receiver, giver *corev1.Service) {
	receiver.Spec = giver.Spec
}

// +kubebuilder:rbac:groups=api.vimana.host,resources=components,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=api.vimana.host,resources=components/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=api.vimana.host,resources=components/finalizers,verbs=update

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.19.0/pkg/reconcile
func (r *ComponentReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	component := &apiv1alpha1.Component{}
	err := r.Get(ctx, req.NamespacedName, component)
	if err != nil {
		if apierrors.IsNotFound(err) {
			logger.Info("Component not found, assumed deleted", "namespace", req.Namespace, "name", req.Name)
			return ctrl.Result{}, nil
		}
		// Error reading the object; re-enqueue the request.
		logger.Error(err, "Failed to get Component", "namespace", req.Namespace, "name", req.Name)
		return ctrl.Result{}, err
	}

	labels := map[string]string{
		labelDomainKey:  component.Spec.Domain,
		labelServerKey:  component.Spec.Server,
		labelVersionKey: component.Spec.Version,
	}
	name := componentName(component.Spec.Domain, component.Spec.Server, component.Spec.Version)
	hashedName := hashed(name)
	deploymentName := prefixed(hashedName, 'd')
	deploymentNamespacedName := types.NamespacedName{
		Name:      deploymentName,
		Namespace: req.Namespace,
	}

	// Generate the corresponding Deployment.
	expectedDeployment := &appsv1.Deployment{
		ObjectMeta: metav1.ObjectMeta{
			Name:      deploymentNamespacedName.Name,
			Namespace: deploymentNamespacedName.Namespace,
			Labels:    labels,
		},
		Spec: appsv1.DeploymentSpec{
			Selector: &metav1.LabelSelector{
				MatchLabels: labels,
			},
			// Note that the replica count is set by `deploymentSpecDiffers` to match the actual value.
			// If the resource does not yet exist, the default value of 1 replica would be used initially.
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{
					Labels: labels,
				},
				Spec: corev1.PodSpec{
					RuntimeClassName: runtimeClassNamePtr,
					Containers: []corev1.Container{
						{
							Name:  grpcContainerName,
							Image: component.Spec.Image,
							Env:   []corev1.EnvVar{},
							// TODO: Switch to IfNotPresent in production.
							//   For local testing, it's import to use Always, because images are effectively mutable;
							//   they may change from run to run while iterating.
							//   In production, however, images are immutable, so we can use IfNotPresent for better performance.
							ImagePullPolicy: corev1.PullAlways,
						},
					},
				},
			},
		},
	}

	// Set the Component as the owner of the Deployment.
	if err = ctrl.SetControllerReference(component, expectedDeployment, r.Scheme); err != nil {
		logger.Error(err, "Failed to set owner reference for Deployment", "namespace", expectedDeployment.Namespace, "name", expectedDeployment.Name)
		return ctrl.Result{}, err
	}

	// Create or Update the Deployment.
	err = ensureResourceHasSpecAndLabels(r.Client, ctx, deploymentNamespacedName, &appsv1.Deployment{}, expectedDeployment, deploymentSpecDiffers, deploymentCopySpec)
	if err != nil {
		return ctrl.Result{}, err
	}

	serviceName := prefixed(hashedName, 's')
	serviceNamespacedName := types.NamespacedName{
		Name:      serviceName,
		Namespace: req.Namespace,
	}

	// Generate the corresponding Service.
	expectedService := &corev1.Service{
		ObjectMeta: metav1.ObjectMeta{
			Name:      serviceNamespacedName.Name,
			Namespace: serviceNamespacedName.Namespace,
			Labels:    labels,
		},
		Spec: corev1.ServiceSpec{
			Ports: []corev1.ServicePort{
				{
					Port:        grpcPortNumber,
					AppProtocol: &grpcAppProtocol,
				},
			},
			Selector: labels,
		},
	}

	// Set the Component as the owner of the Service.
	if err = ctrl.SetControllerReference(component, expectedService, r.Scheme); err != nil {
		logger.Error(err, "Failed to set owner reference for Service", "namespace", expectedService.Namespace, "name", expectedService.Name)
		return ctrl.Result{}, err
	}

	// Create or Update the Service.
	err = ensureResourceHasSpecAndLabels(r.Client, ctx, serviceNamespacedName, &corev1.Service{}, expectedService, serviceSpecDiffers, serviceCopySpec)
	if err != nil {
		return ctrl.Result{}, err
	}

	return ctrl.Result{}, nil
}

// SetupWithManager sets up the controller with the Manager.
func (r *ComponentReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&apiv1alpha1.Component{}).
		Owns(&corev1.Service{}).
		Owns(&appsv1.Deployment{}).
		Complete(r)
}
