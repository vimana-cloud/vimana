package v1alpha1

import (
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// NOTE: json tags are required.  Any new fields you add must have json tags for the fields to be serialized.

// ComponentSpec defines the desired state of Component
type ComponentSpec struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// Version of the component.
	// Must be a valid semantic version string.
	// Must be unique for the server.
	Version string `json:"version"`

	// ID of the server to which this component belongs.
	Server string `json:"server"`

	// ID of the domain to which this server belongs.
	Domain string `json:"domain"`

	// Image URL.
	Image string `json:"image"`
}

// ComponentStatus defines the observed state of Component
type ComponentStatus struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// Status conditions of the Component instance.
	// +operator-sdk:csv:customresourcedefinitions:type=status
	Conditions []metav1.Condition `json:"conditions,omitempty" patchStrategy:"merge" patchMergeKey:"type" protobuf:"bytes,1,rep,name=conditions"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status

// Component is the Schema for the components API
type Component struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   ComponentSpec   `json:"spec,omitempty"`
	Status ComponentStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// ComponentList contains a list of Component
type ComponentList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Component `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Component{}, &ComponentList{})
}

// Return a pointer to the slice of conditions for this resource.
func (resource *Component) GetConditions() *[]metav1.Condition {
	return &resource.Status.Conditions
}
