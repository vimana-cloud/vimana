package controller

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"reflect"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"
)

const (
	// conditionTypeAvailable represents the steady-state existing status of a resource.
	conditionTypeAvailable = "Available"

	labelDomainKey  = "vimana.host/domain"
	labelServerKey  = "vimana.host/server"
	labelVersionKey = "vimana.host/version"
)

const (
	// Name of the runtime class used for all Vimana pods.
	// This is the name that's visible in the API.
	runtimeClassName = "workd-runtime"
	// Name of the runtime handler used for all Vimana pods.
	// This is the value that's passed to the container runtime in the RunPodSandbox request.
	runtimeHandlerName = "workd-handler"
	// The port number used for all gRPC backend servers.
	grpcPortNumber = 80
)

// Extend the interface of a generic K8s object
// with extra methods to facilitate the operator pattern.
type ApiResource interface {
	client.Object

	// Return a (mutable) pointer to the slice of conditions for this resource.
	GetConditions() *[]metav1.Condition
}

// Given the name of a Vimana resource,
// return the name of the corresponding Gateway resource that would be created by the controller.
func gatewayName(vimanaName string) string {
	return vimanaName + ".gateway"
}

// Return the canonical domain name of a domain, derived from the unique ID.
func canonicalDomain(domainId string) string {
	return fmt.Sprintf("%s.app.vimana.host", domainId)
}

// Return the name of the component.
func componentName(domainId, serverId, version string) string {
	return fmt.Sprintf("%s:%s@%s", domainId, serverId, version)
}

// Return the hex-encoded SHA-224 hash of a string.
// The result always contains 56 hexadecimal characters.
func hashed(name string) string {
	hash := sha256.Sum224([]byte(name))
	return hex.EncodeToString(hash[:])
}

// Add a prefix of the form `<prefix>-` to a string.
// When passed a value returned by `hashed` and an alphabetical prefix,
// this function is guaranteed to return a valid K8s resource name,
// which must start with an alphabetical character
// and contain only at most 64 alphanumeric characters and dashes.
func prefixed(content string, prefix rune) string {
	return fmt.Sprintf("%c-%s", prefix, content)
}

// ensureClusterResource ensures a cluster-scoped resource exists by creating it if not found.
// It does not update existing resources.
func ensureClusterResourceExists(
	client client.Client, ctx context.Context, name string, nullResource, defaultResource client.Object,
) error {
	logger := log.FromContext(ctx)
	err := client.Get(ctx, types.NamespacedName{Name: name}, nullResource)
	if err != nil {
		if apierrors.IsNotFound(err) {
			// Create it if it doesn't exist.
			err = client.Create(ctx, defaultResource)
			if err != nil {
				logger.Error(err, "Failed to create resource", "name", name)
			}
		} else {
			// Error reading the object; re-enqueue the request.
			logger.Error(err, "Failed to get resource", "name", name)
		}
	}
	return err
}

// ensureResourceHasSpecAndLabels ensures a namespaced resource exists
// and updates it if the actual spec differs from the expected spec.
func ensureResourceHasSpecAndLabels[T client.Object](
	client client.Client,
	ctx context.Context,
	namespacedName types.NamespacedName,
	actual, expected T,
	specDiffers func(actual, expected T) bool,
	copySpec func(receiver, giver T),
) error {
	logger := log.FromContext(ctx)
	err := client.Get(ctx, namespacedName, actual)
	if err != nil {
		if apierrors.IsNotFound(err) {
			// Create it if it doesn't exist.
			err = client.Create(ctx, expected)
			if err != nil {
				logger.Error(err, "Failed to create resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
			}
		} else {
			// Error reading the object; re-enqueue the request.
			logger.Error(err, "Failed to get resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
		}
	} else {
		// Only update the resource if it differs from expected.
		needsUpdate := false
		if specDiffers(actual, expected) {
			copySpec(actual, expected)
			needsUpdate = true
		}
		expectedLabels := expected.GetLabels()
		if !reflect.DeepEqual(actual.GetLabels(), expectedLabels) {
			actual.SetLabels(expectedLabels)
			needsUpdate = true
		}
		if needsUpdate {
			err = client.Update(ctx, actual)
			if err != nil {
				logger.Error(err, "Failed to update resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
			}
		}
	}
	return err
}

// updateAvailabilityStatus updates a K8s condition status called "Available" on a resource.
func updateAvailabilityStatus(
	client client.Client,
	ctx context.Context,
	resource ApiResource,
	status metav1.ConditionStatus,
	reason, message string,
) error {
	logger := log.FromContext(ctx)
	meta.SetStatusCondition(
		resource.GetConditions(),
		metav1.Condition{
			Type:    conditionTypeAvailable,
			Status:  status,
			Reason:  reason,
			Message: message,
		},
	)
	err := client.Status().Update(ctx, resource)
	if err != nil {
		logger.Error(err, "Failed to update resource status", "namespace", resource.GetNamespace(), "name", resource.GetName())
	} else {
		// Re-fetch the CR after updating the status.
		// It will almost certainly just hit the cache,
		// but this can help avoid errors that say
		// "the object has been modified, please apply your changes to the latest version and try again".
		namespacedName := types.NamespacedName{Name: resource.GetName(), Namespace: resource.GetNamespace()}
		err = client.Get(ctx, namespacedName, resource)
		if err != nil {
			logger.Error(err, "Failed to re-fetch resource after status update", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
		}
	}
	return err
}

// ensureResourceDeleted ensures a resource does not exist.
func ensureResourceDeleted(client client.Client, ctx context.Context, namespacedName types.NamespacedName, resource client.Object) error {
	logger := log.FromContext(ctx)
	err := client.Get(ctx, namespacedName, resource)
	if err != nil {
		if apierrors.IsNotFound(err) {
			// The resource already does not exist.
			return nil
		}
		// Some other error occurred.
		logger.Error(err, "Failed to look up resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
	} else if err = client.Delete(ctx, resource); err != nil {
		logger.Error(err, "Failed to delete resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
	}
	return err
}
