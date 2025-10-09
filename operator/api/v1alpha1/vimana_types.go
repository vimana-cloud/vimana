package v1alpha1

import (
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// NOTE: json tags are required.
//   Any new fields you add must have json tags for the fields to be serialized.

// VimanaSpec defines the desired state of a Vimana.
type VimanaSpec struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// List of names of regions that this cluster is considered a part of.
	// The cluster will only run pods and only host data
	// that are cleared for at least 1 of these regions.
	Regions []string `json:"regions,omitempty"`

	// Hostname and optional port of the image registry
	// used for all component images within this Vimana cluster.
	Registry string `json:"registry,omitempty"`
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
