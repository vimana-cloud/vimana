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

var _ = Describe("Domain Controller", func() {
	Context("When reconciling a resource", func() {
		const namespace = "default"
		const domainId = "0123456789abcdef0123456789abcdef"
		const vimanaId = "the-vimana"
		aliases := []string{"example.com", "api.example.fersher.net"}
		regions := []string{"/us-east", "gcp/us-west1"}
		failover := []string{"backup.example.fersher.net"}

		ctx := context.Background()

		typeNamespacedName := types.NamespacedName{
			Name:      domainId,
			Namespace: namespace,
		}
		domain := &apiv1alpha1.Domain{}
		servers := []*apiv1alpha1.Server{}

		BeforeEach(func() {
			By("creating the custom resource for the Kind Domain")
			err := k8sClient.Get(ctx, typeNamespacedName, domain)
			if err != nil && errors.IsNotFound(err) {
				resource := &apiv1alpha1.Domain{
					ObjectMeta: metav1.ObjectMeta{
						Name:      domainId,
						Namespace: namespace,
					},
					Spec: apiv1alpha1.DomainSpec{
						Id:       domainId,
						Vimana:   vimanaId,
						Aliases:  aliases,
						Regions:  regions,
						Failover: failover,
						Grpc: apiv1alpha1.DomainGrpc{
							Reflection: apiv1alpha1.GrpcReflection{
								// "All but nothing": enable reflection for all services.
								AllBut: []string{},
							},
						},
						OpenApi: true,
					},
				}
				Expect(k8sClient.Create(ctx, resource)).To(Succeed())
			}
			// Reset servers list
			servers = []*apiv1alpha1.Server{}
		})

		AfterEach(func() {
			// Cleanup servers
			for _, server := range servers {
				_ = k8sClient.Delete(ctx, server)
			}
			servers = []*apiv1alpha1.Server{}

			resource := &apiv1alpha1.Domain{}
			err := k8sClient.Get(ctx, typeNamespacedName, resource)
			Expect(err).NotTo(HaveOccurred())

			By("Cleanup the specific resource instance Domain")
			Expect(k8sClient.Delete(ctx, resource)).To(Succeed())
		})

		It("should successfully reconcile the resource with no servers", func() {
			By("creating a GRPCRoute with only hostnames and no rules")
			controllerReconciler := &DomainReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())

			grpcRoute := &gwapi.GRPCRoute{}
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      domainId,
				Namespace: namespace,
			}, grpcRoute)
			Expect(err).To(BeNil(), "Expected GRPCRoute to exist")
			Expect(grpcRoute.ObjectMeta.OwnerReferences).To(HaveLen(1), "Expected GRPCRoute to have 1 owner")
			Expect(grpcRoute.ObjectMeta.OwnerReferences[0].Kind).To(Equal("Domain"))
			Expect(grpcRoute.ObjectMeta.OwnerReferences[0].Name).To(Equal(domainId))

			// Verify hostnames include canonical domain and aliases
			Expect(grpcRoute.Spec.Hostnames).To(HaveLen(3))
			Expect(grpcRoute.Spec.Hostnames).To(ContainElements(
				gwapi.Hostname(domainId+".app.vimana.host"),
				gwapi.Hostname("example.com"),
				gwapi.Hostname("api.example.fersher.net"),
			))

			// Verify no rules since no servers
			Expect(grpcRoute.Spec.Rules).To(BeEmpty())

			// Verify parent reference
			Expect(grpcRoute.Spec.ParentRefs).To(HaveLen(1))
			Expect(grpcRoute.Spec.ParentRefs[0].Name).To(Equal(gwapi.ObjectName("the-vimana.gateway")))
		})

		It("should successfully reconcile with servers and create routing rules", func() {
			By("creating servers with services and version weights")
			controllerReconciler := &DomainReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}

			// Create multiple servers, each with multiple versions
			server1 := &apiv1alpha1.Server{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-server-1",
					Namespace: namespace,
					Labels: map[string]string{
						// Must have the same domain as the other server.
						labelDomainKey: domainId,
					},
				},
				Spec: apiv1alpha1.ServerSpec{
					Id:       "my-server",
					Services: []string{"example.grpc.Service1", "example.grpc.Service2"},
					VersionWeights: map[string]int32{
						"1.0.0": 80,
						"2.1.0": 20,
					},
				},
			}
			server2 := &apiv1alpha1.Server{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-server-2",
					Namespace: namespace,
					Labels: map[string]string{
						// Must have the same domain as the other server.
						labelDomainKey: domainId,
					},
				},
				Spec: apiv1alpha1.ServerSpec{
					Id:       "a-servier-server",
					Services: []string{"some.other.Service"},
					VersionWeights: map[string]int32{
						"1.1.1": 500,
						"2.2.2": 1,
					},
				},
			}
			Expect(k8sClient.Create(ctx, server1)).To(Succeed())
			Expect(k8sClient.Create(ctx, server2)).To(Succeed())
			servers = append(servers, server1, server2)

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())

			grpcRoute := &gwapi.GRPCRoute{}
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      domainId,
				Namespace: namespace,
			}, grpcRoute)
			Expect(err).To(BeNil(), "Expected GRPCRoute to exist")
			Expect(grpcRoute.ObjectMeta.Name).To(Equal(domainId))
			Expect(grpcRoute.ObjectMeta.Namespace).To(Equal(namespace))
			Expect(grpcRoute.ObjectMeta.Labels).To(Equal(map[string]string{
				"vimana.host/domain": domainId,
			}))
			Expect(grpcRoute.Spec).To(Equal(gwapi.GRPCRouteSpec{
				CommonRouteSpec: gwapi.CommonRouteSpec{
					ParentRefs: []gwapi.ParentReference{
						{
							Group:       (*gwapi.Group)(ptr.To("gateway.networking.k8s.io")),
							Kind:        (*gwapi.Kind)(ptr.To("Gateway")),
							Namespace:   nil,
							Name:        "the-vimana.gateway",
							SectionName: nil,
							Port:        nil,
						},
					},
				},
				Hostnames: []gwapi.Hostname{
					"0123456789abcdef0123456789abcdef.app.vimana.host",
					"example.com",
					"api.example.fersher.net",
				},
				Rules: []gwapi.GRPCRouteRule{
					{
						Name: nil,
						Matches: []gwapi.GRPCRouteMatch{
							{
								Method: &gwapi.GRPCMethodMatch{
									Type:    (*gwapi.GRPCMethodMatchType)(ptr.To("Exact")),
									Service: ptr.To("example.grpc.Service1"),
									Method:  nil,
								},
								Headers: nil,
							},
							{
								Method: &gwapi.GRPCMethodMatch{
									Type:    (*gwapi.GRPCMethodMatchType)(ptr.To("Exact")),
									Service: ptr.To("example.grpc.Service2"),
									Method:  nil,
								},
								Headers: nil,
							},
						},
						Filters: nil,
						BackendRefs: []gwapi.GRPCBackendRef{
							{
								BackendRef: gwapi.BackendRef{
									BackendObjectReference: gwapi.BackendObjectReference{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Service")),
										Name:      "s-d69370231670aee9de1c6577abeff97735b4ee1b12365f4a94158653",
										Namespace: nil,
										Port:      ptr.To(gwapi.PortNumber(80)),
									},
									Weight: ptr.To(int32(80)),
								},
								Filters: nil,
							},
							{
								BackendRef: gwapi.BackendRef{
									BackendObjectReference: gwapi.BackendObjectReference{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Service")),
										Name:      "s-4b691f75c60a7dd2bb301c485fbd5df8a995792ee5c4cbe48af8c262",
										Namespace: nil,
										Port:      ptr.To(gwapi.PortNumber(80)),
									},
									Weight: ptr.To(int32(20)),
								},
								Filters: nil,
							},
						},
						SessionPersistence: nil,
					},
					{
						Name: nil,
						Matches: []gwapi.GRPCRouteMatch{
							{
								Method: &gwapi.GRPCMethodMatch{
									Type:    (*gwapi.GRPCMethodMatchType)(ptr.To("Exact")),
									Service: ptr.To("some.other.Service"),
									Method:  nil,
								},
								Headers: nil,
							},
						},
						Filters: nil,
						BackendRefs: []gwapi.GRPCBackendRef{
							{
								BackendRef: gwapi.BackendRef{
									BackendObjectReference: gwapi.BackendObjectReference{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Service")),
										Name:      "s-d165862d33209d230ab761a64a563bd475dad7ed281122b8d07c4cf7",
										Namespace: nil,
										Port:      ptr.To(gwapi.PortNumber(80)),
									},
									Weight: ptr.To(int32(500)),
								},
								Filters: nil,
							},
							{
								BackendRef: gwapi.BackendRef{
									BackendObjectReference: gwapi.BackendObjectReference{
										Group:     (*gwapi.Group)(ptr.To("")),
										Kind:      (*gwapi.Kind)(ptr.To("Service")),
										Name:      "s-7eade10975f18214cfc46de6e6870d653f480137851020cc6e8972f9",
										Namespace: nil,
										Port:      ptr.To(gwapi.PortNumber(80)),
									},
									Weight: ptr.To(int32(1)),
								},
								Filters: nil,
							},
						},
					},
				},
			},
			))
		})

		It("should update GRPCRoute when server is added", func() {
			By("reconciling with no servers initially")
			controllerReconciler := &DomainReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})
			Expect(err).NotTo(HaveOccurred())

			grpcRoute := &gwapi.GRPCRoute{}
			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      domainId,
				Namespace: namespace,
			}, grpcRoute)
			Expect(err).To(BeNil())
			Expect(grpcRoute.Spec.Rules).To(BeEmpty())

			By("adding a server")
			server := &apiv1alpha1.Server{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "new-server",
					Namespace: namespace,
					Labels: map[string]string{
						labelDomainKey: domainId,
					},
				},
				Spec: apiv1alpha1.ServerSpec{
					Id:       "new-server",
					Services: []string{"example.grpc.NewService"},
					VersionWeights: map[string]int32{
						"v1": 100,
					},
				},
			}
			Expect(k8sClient.Create(ctx, server)).To(Succeed())

			By("reconciling again with the new server")
			_, err = controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})
			Expect(err).NotTo(HaveOccurred())

			err = k8sClient.Get(ctx, types.NamespacedName{
				Name:      domainId,
				Namespace: namespace,
			}, grpcRoute)
			Expect(err).To(BeNil())
			Expect(grpcRoute.Spec.Rules).To(HaveLen(1))
			Expect(grpcRoute.Spec.Rules[0].Matches).To(HaveLen(1))
			Expect(*grpcRoute.Spec.Rules[0].Matches[0].Method.Service).To(Equal("example.grpc.NewService"))

			Expect(k8sClient.Delete(ctx, server)).To(Succeed())
		})

		It("should fail is multiple services have the same service", func() {
			By("creating servers that claim the same service name")
			controllerReconciler := &DomainReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}

			server1 := &apiv1alpha1.Server{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-server-1",
					Namespace: namespace,
					Labels:    map[string]string{labelDomainKey: domainId},
				},
				Spec: apiv1alpha1.ServerSpec{
					Id:             "my-server",
					Services:       []string{"example.grpc.Service", "this.is.the.conflicting.Service"},
					VersionWeights: map[string]int32{"1.0.0": 100},
				},
			}
			server2 := &apiv1alpha1.Server{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-server-2",
					Namespace: namespace,
					Labels:    map[string]string{labelDomainKey: domainId},
				},
				Spec: apiv1alpha1.ServerSpec{
					Id:             "a-servier-server",
					Services:       []string{"example.grpc.OtherService", "this.is.the.conflicting.Service"},
					VersionWeights: map[string]int32{"2.0.0": 100},
				},
			}
			Expect(k8sClient.Create(ctx, server1)).To(Succeed())
			Expect(k8sClient.Create(ctx, server2)).To(Succeed()) // TODO: This should fail.
			servers = append(servers, server1, server2)

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())
		})
	})
})
