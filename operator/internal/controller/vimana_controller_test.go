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

	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

var _ = Describe("Vimana Controller", func() {
	Context("When reconciling a resource", func() {
		const namespace = "default"
		const resourceName = "test-resource"
		const registry = "some.hosted.registry.somewhere"
		vimanaRegions := []string{"/us-east", "aws/us-east"}
		const domainId = "0123456789abcdef0123456789abcdef"
		domainAliases := []string{"example.com", "foo.bar.whatsittoyouz.net"}
		domainRegions := []string{"aws/us-east"}
		domainFailover := []string{}
		var domainGrpc *apiv1alpha1.DomainGrpc
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
						Regions:  vimanaRegions,
						Registry: registry,
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
				Name:      gatewayName(resourceName),
				Namespace: namespace,
			}, &gwapi.Gateway{})
			Expect(err).NotTo(BeNil(), "Expected Gateway to not exist")
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
					Grpc:     domainGrpc,
					OpenApi:  domainOpenApi,
				},
			}
			Expect(k8sClient.Create(ctx, domain)).To(Succeed())

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())
			gateway := &gwapi.Gateway{}
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      gatewayName(resourceName),
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
							TLS: &gwapi.ListenerTLSConfig{
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
							TLS: &gwapi.ListenerTLSConfig{
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
							TLS: &gwapi.ListenerTLSConfig{
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
					TLS:              nil,
					DefaultScope:     "",
				},
			))
		})
	})
})

func gatewayName(vimanaName string) string {
	return vimanaName + ".gateway"
}
