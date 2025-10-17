package controller

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"

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

	labelDomainKey = "vimana.host/domain"
)

// Return the canonical domain name of a domain, derived from the unique ID.
func canonicalDomain(domainId string) string {
	return fmt.Sprintf("%s.app.vimana.host", domainId)
}

// Return the name of the component.
func componentName(domainId, serverId, version string) string {
	return fmt.Sprintf("%s:%s@%s", domainId, serverId, version)
}

// Return a valid K8s resource name
// that is deterministically derived from (and "uniquely" identifies) the provided content string.
// The prefix must be an alphabetical character.
func contentAddressedName(content string, prefix rune) string {
	hash := sha256.Sum224([]byte(content))
	return fmt.Sprintf("%c-%s", prefix, hex.EncodeToString(hash[:]))
}

// Extend the interface of a generic K8s object
// with extra methods to facilitate the operator pattern.
type ApiResource interface {
	client.Object

	// Return a (mutable) pointer to the slice of conditions for this resource.
	GetConditions() *[]metav1.Condition
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

// ensureResourceHasSpec ensures a namespaced resource exists
// and updates it if the actual spec differs from the expected spec.
func ensureResourceHasSpec[T client.Object](
	client client.Client,
	ctx context.Context,
	namespacedName types.NamespacedName,
	actual, expected T,
	specDiffers func(left, right T) bool,
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
	} else if specDiffers(actual, expected) {
		// Only update the resource if it differs from expected.
		copySpec(actual, expected)
		err = client.Update(ctx, actual)
		if err != nil {
			logger.Error(err, "Failed to update resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
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
		// The resource exists. Delete it.
		logger.Error(err, "Failed to delete resource", "namespace", namespacedName.Namespace, "name", namespacedName.Name)
	}
	return err
}
