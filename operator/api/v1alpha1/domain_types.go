package v1alpha1

import (
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// EDIT THIS FILE!  THIS IS SCAFFOLDING FOR YOU TO OWN!
// NOTE: json tags are required.  Any new fields you add must have json tags for the fields to be serialized.

// DomainSpec defines the desired state of a Domain.
type DomainSpec struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// Auto-generated unique ID of the domain, as a hex-encoded string.
	Id string `json:"id"`

	// List of alias domain names.
	Aliases []string `json:"aliases,omitempty"`

	// Subset of regions in which servers within this domain may run.
	// If empty, they could run anywhere globally.
	Regions []string `json:"regions,omitempty"`

	// List of domain names to forward traffic to in case of an outage.
	Failover []string `json:"failover,omitempty"`

	// gRPC-specific configuration for the domain.
	Grpc DomainGrpc `json:"grpc,omitempty"`

	// Provide an auto-generated OpenAPI Description at `/.well-known/schema.json`
	// covering all the HTTP-transcoded methods of all the servers in the domain.
	OpenApi bool `json:"openApi,omitempty"`
}

// DomainGrpc defines the desired state of the gRPC settings of a Domain.
type DomainGrpc struct {
	// Enable gRPC reflection for some subset of services within this domain.
	// Serves the special `grpc.reflection.v1.ServerReflection` service
	// which provides a spec of the specified services.
	Reflection GrpcReflection `json:"reflection,omitempty"`
}

// GrpcReflection defines the desired state of the gRPC reflection settings of a Domain.
type GrpcReflection struct {
	// Only enable reflection for these full-named services.
	// Incompatible with `allBut`.
	// The default is empty (reflection never enabled).
	NoneBut []string `json:"noneBut,omitempty"`
	// Enable reflection for every service except these full-named services.
	// Incompatible with `noneBut`.
	AllBut []string `json:"allBut,omitempty"`
}

// DomainStatus defines the observed state of a Domain.
type DomainStatus struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// Status conditions of the Vimana instance.
	// +operator-sdk:csv:customresourcedefinitions:type=status
	Conditions []metav1.Condition `json:"conditions,omitempty" patchStrategy:"merge" patchMergeKey:"type" protobuf:"bytes,1,rep,name=conditions"`
}

// +kubebuilder:object:root=true

// Domain is the Schema for the domains API
// +kubebuilder:subresource:status
type Domain struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   DomainSpec   `json:"spec,omitempty"`
	Status DomainStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// DomainList contains a list of Domain
type DomainList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Domain `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Domain{}, &DomainList{})
}
