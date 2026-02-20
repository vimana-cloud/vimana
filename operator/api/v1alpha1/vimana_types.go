package v1alpha1

import (
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// NOTE: json tags are required.
//   Any new fields you add must have json tags for the fields to be serialized.

// VimanaSpec defines the desired state of a Vimana.
type VimanaSpec struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// If specified, every Domain within the Vimana is assigned a canonical domain
	// by using the Domain ID to subdomain this "superdomain".
	// For example, if the canonical superdomain is `vimana.host`
	// and a Domain has ID `00000000000000000000000000000000`
	// then the canonical domain for that Domain
	// would be `00000000000000000000000000000000.vimana.host`.
	CanonicalSuperdomain string `json:"canonicalSuperdomain,omitempty"`

	// Reference to a cert-manager Issuer or ClusterIssuer
	// used to provision TLS certificates for the Gateway.
	// When set, the Gateway is annotated so that cert-manager
	// automatically creates Certificate resources for each listener.
	// Only `Name` and `Kind` are used.
	IssuerRef *corev1.ObjectReference `json:"issuerRef,omitempty"`
}

// VimanaStatus defines the observed state of a Vimana cluster.
type VimanaStatus struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// Status conditions of the Vimana instance.
	// +operator-sdk:csv:customresourcedefinitions:type=status
	Conditions []metav1.Condition `json:"conditions,omitempty" patchStrategy:"merge" patchMergeKey:"type" protobuf:"bytes,1,rep,name=conditions"`
}

// +kubebuilder:object:root=true

// Vimana is the Schema for the vimanas API.
// +kubebuilder:subresource:status
type Vimana struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   VimanaSpec   `json:"spec,omitempty"`
	Status VimanaStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// VimanaList contains a list of Vimana
type VimanaList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Vimana `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Vimana{}, &VimanaList{})
}

// Return a pointer to the slice of conditions for this resource.
func (resource *Vimana) GetConditions() *[]metav1.Condition {
	return &resource.Status.Conditions
}
