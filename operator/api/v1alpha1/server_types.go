/*
Copyright 2025.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package v1alpha1

import (
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// EDIT THIS FILE!  THIS IS SCAFFOLDING FOR YOU TO OWN!
// NOTE: json tags are required.  Any new fields you add must have json tags for the fields to be serialized.

// ServerSpec defines the desired state of Server
type ServerSpec struct {
	// Important: Run `bazel run //operator:generate` to regenerate code
	//   after modifying this file.

	// User-provided unique ID of the server.
	Id string `json:"id"`

	// List of fully-qualified gRPC service names served by this server.
	Services []string `json:"services,omitempty"`

	// Whether gRPC reflection is enabled for all the services on this server.
	// If this is enabled,
	// then neither `grpc.reflection.v1.ServerReflection` nor `grpc.reflection.v1alpha1.ServerReflection`
	// may be specified under `Services`.
	Reflection bool `json:"reflection,omitempty"`

	// Authentication configuration for the server.
	Auth ServerAuth `json:"auth"`

	// Map from feature flag names to configurations.
	Features map[string]FeatureFlag `json:"features,omitempty"`

	// Map from version strings to traffic weights.
	// The traffic proportion is the weight divided by the total of all weights.
	VersionWeights map[string]int32 `json:"versionWeights,omitempty"`
}

type ServerAuth struct {
	// URLs of JSON web key set that can be used to validate JWTs on incoming requests.
	Jwks []string `json:"jwks,omitempty"`
}

type FeatureFlag struct {
	// TODO: Define feature flags.
	//   "some-bool-flag":
	//     # Each case is defined by a value and a set of conditions.
	//     # Evaluate the case in order and use the value of the first one whose conditions match.
	//     - boolean: true
	//       conditions:
	//         # At least one top-level condition must match (they are "OR-joined").
	//         # This condition matches people with a verified email address on 'example.com'
	//         # according to any attached JWT attached to the request.
	//         # This kind of filter is only useful if you specify JWKS to verify JWTs.
	//         - hasEmail: "*@example.com"
	//           # Conditions can be nested 1 level deep.
	//           # At this level, they are "AND-joined";
	//           # all must match for the overall (top-level) condition to match.
	//           # This condition matches people from 'sometimes.com', but only half the time.
	//         - - hasEmail: "*@sometimes.com"
	//           - random: 50%
	//     # Each case must use the same type.
	//     # The final case must have no conditions.
	//     - boolean: false
	//   "some-string-flag":
	//     - string: "good"
	//       conditions:
	//         ...
}

// ServerStatus defines the observed state of Server
type ServerStatus struct {
	// INSERT ADDITIONAL STATUS FIELD - define observed state of cluster
	// Important: Run "make" to regenerate code after modifying this file
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status

// Server is the Schema for the servers API
type Server struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   ServerSpec   `json:"spec,omitempty"`
	Status ServerStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// ServerList contains a list of Server
type ServerList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Server `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Server{}, &ServerList{})
}
