package controller

import (
	"context"
	"encoding/json"
	"fmt"
	"reflect"

	envoygateway "github.com/envoyproxy/gateway/api/v1alpha1"
	apiextensionsv1 "k8s.io/apiextensions-apiserver/pkg/apis/apiextensions/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
	gwapi "sigs.k8s.io/gateway-api/apis/v1"
	gwapiv1 "sigs.k8s.io/gateway-api/apis/v1"

	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

const (
	// `proto_descriptor_bin` is represented by a `ConfigMap` which cannot exceed 1 MiB.
	// https://kubernetes.io/docs/concepts/configuration/configmap/
	protoDescriptorBinSizeLimit = 1024 * 1024
)

var (
	// Turn this into a variable so we can take its address.
	grpcPortNumberForGateway = gwapi.PortNumber(grpcPortNumber)

	// K8s resource kind for a Service.
	serviceKind = gwapi.Kind("Service")

	// Make this a variable so that it has an address and we can get a pointer to it.
	exactMethodMatch = gwapi.GRPCMethodMatchExact
)

// DomainReconciler reconciles a Domain object.
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

// Return true iff the two objects are *not* equal.
func envoyPatchPolicySpecDiffers(actual, expected *envoygateway.EnvoyPatchPolicy) bool {
	return !reflect.DeepEqual(actual.Spec, expected.Spec)
}

// Mutate the "spec" value of the receiver to match that of the other object.
func envoyPatchPolicyCopySpec(receiver, giver *envoygateway.EnvoyPatchPolicy) {
	receiver.Spec = giver.Spec
}

// +kubebuilder:rbac:groups=api.vimana.host,resources=domains,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=api.vimana.host,resources=domains/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=api.vimana.host,resources=domains/finalizers,verbs=update
// +kubebuilder:rbac:groups=api.vimana.host,resources=servers,verbs=get;list;watch
// +kubebuilder:rbac:groups=gateway.networking.k8s.io,resources=grpcroutes,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=gateway.envoyproxy.io,resources=envoypatchpolicies,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=cert-manager.io,resources=certificates,verbs=get;create

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.23.1/pkg/reconcile
func (r *DomainReconciler) Reconcile(ctx context.Context, request ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	domain := &apiv1alpha1.Domain{}
	err := r.Get(ctx, request.NamespacedName, domain)
	if err != nil {
		if apierrors.IsNotFound(err) {
			logger.Info("Domain not found, assumed deleted", "namespace", request.Namespace, "name", request.Name)
			return ctrl.Result{}, nil
		}
		// Error reading the object; re-enqueue the request.
		logger.Error(err, "Failed to get Domain", "namespace", request.Namespace, "name", request.Name)
		return ctrl.Result{}, err
	}

	// Set the status as Unknown when no status is available.
	if len(domain.Status.Conditions) == 0 {
		err = updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionUnknown, "Reconciling", "Starting reconciliation")
		if err != nil {
			return ctrl.Result{}, err
		}
	}

	// Get the Vimana above the domain.
	vimana := &apiv1alpha1.Vimana{}
	vimanaNamespacedName := types.NamespacedName{
		Name:      domain.Spec.Vimana,
		Namespace: request.Namespace,
	}
	err = r.Get(ctx, vimanaNamespacedName, vimana)
	if err != nil {
		logger.Error(err, "Failed to get Vimana for Domain", "namespace", request.Namespace, "vimana", domain.Spec.Vimana)
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "VimanaNotFound", "Failed to find parent Vimana")
		return ctrl.Result{}, err
	}

	// List all the servers under the domain.
	servers := &apiv1alpha1.ServerList{}
	err = r.List(ctx, servers, client.InNamespace(request.Namespace), client.MatchingLabels{labelDomainKey: domain.Spec.Id})
	if err != nil {
		logger.Error(err, "Failed to list Servers", "namespace", request.Namespace, "domain", domain.Spec.Id)
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "ServerList", "Failed to list servers within the domain")
		return ctrl.Result{}, err
	}

	err = r.reconcileGrpcRoute(ctx, request, vimana, domain, servers)
	if err != nil {
		return ctrl.Result{}, err
	}

	err = r.reconcileGatewayJsonPatch(ctx, request, vimana, domain, servers)
	if err != nil {
		return ctrl.Result{}, err
	}

	err = r.reconcileCertificates(ctx, request, vimana, domain)
	if err != nil {
		return ctrl.Result{}, err
	}

	err = updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionTrue, "Reconciled", "Successfully reconciled domain")
	return ctrl.Result{}, err
}

func (r *DomainReconciler) reconcileGrpcRoute(
	ctx context.Context,
	request ctrl.Request,
	vimana *apiv1alpha1.Vimana,
	domain *apiv1alpha1.Domain,
	servers *apiv1alpha1.ServerList,
) error {
	logger := log.FromContext(ctx)

	hostnames := make([]gwapi.Hostname, 0, len(domain.Spec.Aliases)+1)
	for domainName := range domainNames(domain, vimana) {
		hostnames = append(hostnames, gwapi.Hostname(domainName))
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

		// Sort the versions so there's a deterministic ordering to the backend refs in the generated GRPCRoute.
		// This may help to avoid unnecessary updates when the generated GRPCRoute is identical to the pre-existing version
		// except for the ordering of the backend refs (whose order does not matter).
		versions := sortedKeys(server.Spec.VersionWeights)
		backendRefs := make([]gwapi.GRPCBackendRef, 0, len(server.Spec.VersionWeights))
		for _, version := range versions {
			weight := server.Spec.VersionWeights[version]
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
		Namespace: request.Namespace,
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
	err := ctrl.SetControllerReference(domain, expectedGrpcRoute, r.Scheme)
	if err != nil {
		logger.Error(err, "Failed to set owner reference for GRPCRoute", "namespace", expectedGrpcRoute.Namespace, "name", expectedGrpcRoute.Name)
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "GRPCRouteOwner", "Failed to set owner for GRPCRoute")
		return err
	}

	// Create or Update the GRPCRoute.
	err = ensureResourceHasSpecAndLabels(r.Client, ctx, grpcRouteNamespacedName, &gwapi.GRPCRoute{}, expectedGrpcRoute, grpcRouteSpecDiffers, grpcRouteCopySpec)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "GRPCRouteUpsert", "Failed to update GRPCRoute for domain")
		return err
	}

	return nil
}

func (r *DomainReconciler) reconcileGatewayJsonPatch(
	ctx context.Context,
	request ctrl.Request,
	vimana *apiv1alpha1.Vimana,
	domain *apiv1alpha1.Domain,
	servers *apiv1alpha1.ServerList,
) error {
	descriptors := make([]string, 0, len(servers.Items))
	services := make([]string, 0, len(servers.Items))
	totalDescriptorLength := 0
	for _, server := range servers.Items {
		if server.Spec.JSON != nil && server.Spec.JSON.ProtoDescriptorBin != "" {
			// `totalDescriptorLength` may be biased larger than the final concatenation
			// due to the fact that values are decoded (with padding), then concatenated,
			// then re-encoded (with possibly less total padding than before).
			totalDescriptorLength += len(server.Spec.JSON.ProtoDescriptorBin)
			if totalDescriptorLength > protoDescriptorBinSizeLimit {
				updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "ConcatenateDescriptors", "Proto descriptor binaries exceed length limit")
				return fmt.Errorf("Base64-lengths exceed combined limit")
			}
			descriptors = append(descriptors, server.Spec.JSON.ProtoDescriptorBin)
		}
		// TODO: Error if service names are not unique?
		services = append(services, server.Spec.Services...)
	}

	// Due to the nature of the `FileDescriptorProto`,
	// which is basically just a single repeated field,
	// these fields can be trivially concatenated (accounting for base64-encoding).
	// https://github.com/protocolbuffers/protobuf/blob/v33.5/src/google/protobuf/descriptor.proto#L56
	protoDescriptorBin, err := concatenateBase64(descriptors, totalDescriptorLength)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "ConcatenateDescriptors", "Failed to re-encode proto descriptor binaries")
		return err
	}

	patchPolicyName := prefixed(hashed(domain.Spec.Id), 'j')
	patchPolicyNamespacedName := types.NamespacedName{
		Name:      patchPolicyName,
		Namespace: request.Namespace,
	}

	if protoDescriptorBin == "" {
		err = ensureResourceDeleted(r.Client, ctx, patchPolicyNamespacedName, &envoygateway.EnvoyPatchPolicy{})
		if err != nil {
			updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "PatchPolicyDelete", "Failed to delete EnvoyPatchPolicy")
			return err
		}
		return updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionTrue, "Reconciled", "Successfully reconciled server")
	}

	// https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/grpc_json_transcoder_filter
	transcoderConfig := map[string]any{
		"name": "envoy.filters.http.grpc_json_transcoder",
		"typed_config": map[string]any{
			"@type":                "type.googleapis.com/envoy.extensions.filters.http.grpc_json_transcoder.v3.GrpcJsonTranscoder",
			"proto_descriptor_bin": protoDescriptorBin,
			"services":             services,
			// Print options can be used to configure things like whitespace,
			// omission of zero-valued primitive fields, enum value representation, and field name conversion.
			// Use the defaults for now.
			// https://github.com/envoyproxy/envoy/blob/v1.37.0/api/envoy/config/filter/http/transcoder/v2/transcoder.proto#L23
			//"print_options": map[string]any{},
		},
	}

	transcoderJSON, err := json.Marshal(transcoderConfig)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "PatchPolicyBuild", "Failed to marshal transcoder config")
		return fmt.Errorf("Failed to marshal transcoder config: %w", err)
	}

	gatewayName := gatewayName(domain.Spec.Vimana)
	listenerName := fmt.Sprintf("%s/%s/https", request.Namespace, gatewayName)

	expectedPatchPolicy := &envoygateway.EnvoyPatchPolicy{
		ObjectMeta: metav1.ObjectMeta{
			Name:      patchPolicyNamespacedName.Name,
			Namespace: patchPolicyNamespacedName.Namespace,
			Labels: map[string]string{
				labelDomainKey: domain.Spec.Id,
			},
		},
		Spec: envoygateway.EnvoyPatchPolicySpec{
			Type: envoygateway.JSONPatchEnvoyPatchType,
			TargetRef: gwapiv1.LocalPolicyTargetReference{
				Group: "gateway.networking.k8s.io",
				Kind:  "Gateway",
				Name:  gwapiv1.ObjectName(gatewayName),
			},
			JSONPatches: []envoygateway.EnvoyJSONPatchConfig{
				{
					Type: envoygateway.ListenerEnvoyResourceType,
					Name: listenerName,
					Operation: envoygateway.JSONPatchOperation{
						Op: "add",
						// Insert before the router filter.
						Path: ptr.To(
							"/filter_chains/0/filters/0/typed_config/http_filters/0"),
						Value: &apiextensionsv1.JSON{
							Raw: transcoderJSON,
						},
					},
				},
			},
		},
	}

	// Set the Server as the owner of the EnvoyPatchPolicy.
	if err = ctrl.SetControllerReference(domain, expectedPatchPolicy, r.Scheme); err != nil {
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "PatchPolicyOwner", "Failed to set owner for EnvoyPatchPolicy")
		return err
	}

	// Create or Update the EnvoyPatchPolicy.
	err = ensureResourceHasSpecAndLabels(r.Client, ctx, patchPolicyNamespacedName, &envoygateway.EnvoyPatchPolicy{}, expectedPatchPolicy, envoyPatchPolicySpecDiffers, envoyPatchPolicyCopySpec)
	if err != nil {
		updateAvailabilityStatus(r.Client, ctx, domain, metav1.ConditionFalse, "PatchPolicyUpsert", "Failed to update EnvoyPatchPolicy")
		return err
	}

	return nil
}

func (r *DomainReconciler) reconcileCertificates(
	ctx context.Context,
	request ctrl.Request,
	vimana *apiv1alpha1.Vimana,
	domain *apiv1alpha1.Domain,
) error {
	if vimana.Spec.IssuerRef == nil {
		return nil
	}

	for domainName := range domainNames(domain, vimana) {
		certName := prefixed(hashed(domainName), 'c')
		namespacedName := types.NamespacedName{
			Name:      certName,
			Namespace: request.Namespace,
		}

		existing := &unstructured.Unstructured{}
		existing.SetGroupVersionKind(schema.GroupVersionKind{
			Group:   "cert-manager.io",
			Version: "v1",
			Kind:    "Certificate",
		})
		err := r.Get(ctx, namespacedName, existing)
		if err == nil {
			continue // already exists
		}
		if !apierrors.IsNotFound(err) {
			return err
		}

		cert := &unstructured.Unstructured{}
		cert.SetGroupVersionKind(schema.GroupVersionKind{
			Group:   "cert-manager.io",
			Version: "v1",
			Kind:    "Certificate",
		})
		cert.SetName(certName)
		cert.SetNamespace(request.Namespace)
		cert.Object["spec"] = map[string]any{
			"secretName": certName,
			"dnsNames":   []any{domainName},
			"issuerRef": map[string]any{
				"name":  vimana.Spec.IssuerRef.Name,
				"kind":  vimana.Spec.IssuerRef.Kind,
				"group": "cert-manager.io",
			},
		}

		if err = ctrl.SetControllerReference(domain, cert, r.Scheme); err != nil {
			return err
		}
		if err = r.Create(ctx, cert); err != nil {
			return err
		}
	}

	return nil
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
		Name:      domainId,
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
