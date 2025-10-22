package controller

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	"k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	gwapi "sigs.k8s.io/gateway-api/apis/v1"

	nodev1 "k8s.io/api/node/v1"
	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

var _ = Describe("Vimana Controller", func() {
	Context("When reconciling a resource", func() {
		const namespace = "default"
		const resourceName = "test-resource"
		const gatewayName = "test-resource.gateway"
		vimanaRegions := []string{"/us-east", "aws/us-east"}
		const domainId = "0123456789abcdef0123456789abcdef"
		domainAliases := []string{"example.com", "foo.bar.whatsittoyouz.net"}
		domainRegions := []string{"aws/us-east"}
		domainFailover := []string{}
		domainOpenApi := false

		ctx := context.Background()

		typeNamespacedName := types.NamespacedName{
			Name:      resourceName,
			Namespace: namespace,
		}
		vimana := &apiv1alpha1.Vimana{}

		BeforeEach(func() {
			By("creating the custom resource for the Kind Vimana")
			err := k8sClient.Get(ctx, typeNamespacedName, vimana)
			if err != nil && errors.IsNotFound(err) {
				resource := &apiv1alpha1.Vimana{
					ObjectMeta: metav1.ObjectMeta{
						Name:      resourceName,
						Namespace: namespace,
					},
					Spec: apiv1alpha1.VimanaSpec{
						Regions: vimanaRegions,
					},
				}

				Expect(k8sClient.Create(ctx, resource)).To(Succeed())
			}
		})

		AfterEach(func() {
			resource := &apiv1alpha1.Vimana{}
			err := k8sClient.Get(ctx, typeNamespacedName, resource)
			Expect(err).NotTo(HaveOccurred())

			By("Cleanup the specific resource instance Vimana")
			Expect(k8sClient.Delete(ctx, resource)).To(Succeed())
		})

		It("should successfully reconcile the resource with no domains", func() {
			By("creating nothing")
			controllerReconciler := &VimanaReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      gatewayName,
				Namespace: namespace,
			}, &gwapi.Gateway{})
			Expect(err).NotTo(BeNil(), "Expected Gateway to *not* exist")
			Expect(errors.IsNotFound(err)).To(BeTrue(), err.Error())
		})

		It("should successfully reconcile the resource with domains", func() {
			By("creating the gateway")
			controllerReconciler := &VimanaReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}
			domain := &apiv1alpha1.Domain{
				ObjectMeta: metav1.ObjectMeta{
					Name:      resourceName,
					Namespace: namespace,
				},
				Spec: apiv1alpha1.DomainSpec{
					Id:       domainId,
					Aliases:  domainAliases,
					Regions:  domainRegions,
					Failover: domainFailover,
					Grpc:     apiv1alpha1.DomainGrpc{},
					OpenApi:  domainOpenApi,
				},
			}
			Expect(k8sClient.Create(ctx, domain)).To(Succeed())
			runtimeClass := &nodev1.RuntimeClass{}
			gatewayClass := &gwapi.GatewayClass{}
			gateway := &gwapi.Gateway{}

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())

			// Verify status conditions
			err = k8sClient.Get(ctx, typeNamespacedName, vimana)
			Expect(err).NotTo(HaveOccurred())
			Expect(vimana.Status.Conditions).To(HaveLen(1))
			condition := vimana.Status.Conditions[0]
			Expect(condition).To(Equal(metav1.Condition{
				Type:               "Available",
				Status:             metav1.ConditionTrue,
				Reason:             "Reconciled",
				Message:            "Successfully reconciled vimana",
				LastTransitionTime: condition.LastTransitionTime, // non-deterministic
			}))

			err = k8sClient.Get(ctx, types.NamespacedName{
				Name: runtimeClassName,
			}, runtimeClass)
			Expect(err).To(BeNil(), "Expected RuntimeClass to exist")
			// Cluster-scoped resources are not given an owner
			// because they could be shared among many Vimana resources (and thus outlive any of them).
			Expect(runtimeClass.ObjectMeta.OwnerReferences).To(BeNil(), "Expected RuntimeClass to have no owner")
			Expect(runtimeClass.Handler).To(Equal(runtimeHandlerName))
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name: gatewayClassName,
			}, gatewayClass)
			Expect(err).To(BeNil(), "Expected GatewayClass to exist")
			// Cluster-scoped resources are not given an owner
			// because they could be shared among many Vimana resources (and thus outlive any of them).
			Expect(gatewayClass.ObjectMeta.OwnerReferences).To(BeNil(), "Expected GatewayClass to have no owner")
			Expect(gatewayClass.Spec).To(Equal(
				gwapi.GatewayClassSpec{
					ControllerName: "gateway.envoyproxy.io/gatewayclass-controller",
					ParametersRef: &gwapi.ParametersReference{
						Group:     "gateway.envoyproxy.io",
						Kind:      "EnvoyProxy",
						Name:      gatewayConfigName,
						Namespace: (*gwapi.Namespace)(ptr.To(gatewayNamespace)),
					},
					Description: ptr.To("Vimana Gateway class"),
				},
			))
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      gatewayName,
				Namespace: namespace,
			}, gateway)
			Expect(err).To(BeNil(), "Expected Gateway to exist")
			Expect(gateway.ObjectMeta.OwnerReferences).To(HaveLen(1), "Expected Gateway to have 1 owner")
			Expect(gateway.ObjectMeta.OwnerReferences).To(Equal(
				[]metav1.OwnerReference{
					{
						APIVersion:         "api.vimana.host/v1alpha1",
						Kind:               "Vimana",
						Name:               resourceName,
						UID:                gateway.ObjectMeta.OwnerReferences[0].UID,
						Controller:         ptr.To(true),
						BlockOwnerDeletion: ptr.To(true),
					},
				},
			))
			Expect(gateway.Spec).To(Equal(
				gwapi.GatewaySpec{
					GatewayClassName: "envoy-gateway",
					Listeners: []gwapi.Listener{
						{
							Name:     "l-5f5f7340abee4e7850fe8911987d123864a38030c401f246f1f0051035aa51c0",
							Hostname: (*gwapi.Hostname)(ptr.To(domainId + ".app.vimana.host")),
							Port:     443,
							Protocol: "HTTPS",
							TLS: &gwapi.GatewayTLSConfig{
								Mode: (*gwapi.TLSModeType)(ptr.To("Terminate")),
								CertificateRefs: []gwapi.SecretObjectReference{
									{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Secret")),
										Name:      "c-5f5f7340abee4e7850fe8911987d123864a38030c401f246f1f0051035aa51c0",
										Namespace: (*gwapi.Namespace)(ptr.To(namespace)),
									},
								},
								Options: nil,
							},
							AllowedRoutes: &gwapi.AllowedRoutes{
								Namespaces: &gwapi.RouteNamespaces{
									From:     (*gwapi.FromNamespaces)(ptr.To("Same")),
									Selector: nil,
								},
								Kinds: []gwapi.RouteGroupKind{
									{
										Group: (*gwapi.Group)(ptr.To("gateway.networking.k8s.io")),
										Kind:  "GRPCRoute",
									},
								},
							},
						},
						{
							Name:     "l-a379a6f6eeafb9a55e378c118034e2751e682fab9f2d30ab13d2125586ce1947",
							Hostname: (*gwapi.Hostname)(ptr.To("example.com")),
							Port:     443,
							Protocol: "HTTPS",
							TLS: &gwapi.GatewayTLSConfig{
								Mode: (*gwapi.TLSModeType)(ptr.To("Terminate")),
								CertificateRefs: []gwapi.SecretObjectReference{
									{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Secret")),
										Name:      "c-a379a6f6eeafb9a55e378c118034e2751e682fab9f2d30ab13d2125586ce1947",
										Namespace: (*gwapi.Namespace)(ptr.To(namespace)),
									},
								},
								Options: nil,
							},
							AllowedRoutes: &gwapi.AllowedRoutes{
								Namespaces: &gwapi.RouteNamespaces{
									From:     (*gwapi.FromNamespaces)(ptr.To("Same")),
									Selector: nil,
								},
								Kinds: []gwapi.RouteGroupKind{
									{
										Group: (*gwapi.Group)(ptr.To("gateway.networking.k8s.io")),
										Kind:  "GRPCRoute",
									},
								},
							},
						},
						{
							Name:     "l-56f044729e8f511482e831dfb2d87b8b2e8affb7464b319b3e6c8237d19c7b2d",
							Hostname: (*gwapi.Hostname)(ptr.To("foo.bar.whatsittoyouz.net")),
							Port:     443,
							Protocol: "HTTPS",
							TLS: &gwapi.GatewayTLSConfig{
								Mode: (*gwapi.TLSModeType)(ptr.To("Terminate")),
								CertificateRefs: []gwapi.SecretObjectReference{
									{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Secret")),
										Name:      "c-56f044729e8f511482e831dfb2d87b8b2e8affb7464b319b3e6c8237d19c7b2d",
										Namespace: (*gwapi.Namespace)(ptr.To(namespace)),
									},
								},
								Options: nil,
							},
							AllowedRoutes: &gwapi.AllowedRoutes{
								Namespaces: &gwapi.RouteNamespaces{
									From:     (*gwapi.FromNamespaces)(ptr.To("Same")),
									Selector: nil,
								},
								Kinds: []gwapi.RouteGroupKind{
									{
										Group: (*gwapi.Group)(ptr.To("gateway.networking.k8s.io")),
										Kind:  "GRPCRoute",
									},
								},
							},
						},
					},
					Addresses:        nil,
					Infrastructure:   nil,
					AllowedListeners: nil,
				},
			))
		})
	})
})
