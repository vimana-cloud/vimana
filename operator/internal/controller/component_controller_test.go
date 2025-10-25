package controller

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/intstr"
	"k8s.io/utils/ptr"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	apiv1alpha1 "vimana.host/operator/api/v1alpha1"
)

var _ = Describe("Component Controller", func() {
	Context("When reconciling a resource", func() {
		const namespace = "default"
		const resourceName = "test-resource"
		const domainId = "0123456789abcdef0123456789abcdef"
		const serverId = "some-id"
		const version = "1.2.3"
		const image = "gcr.io/some/image:latest"
		labels := map[string]string{
			"vimana.host/domain":  domainId,
			"vimana.host/server":  serverId,
			"vimana.host/version": version,
		}

		ctx := context.Background()

		typeNamespacedName := types.NamespacedName{
			Name:      resourceName,
			Namespace: namespace,
		}
		component := &apiv1alpha1.Component{}

		BeforeEach(func() {
			By("creating the custom resource for the Kind Component")
			err := k8sClient.Get(ctx, typeNamespacedName, component)
			if err != nil && errors.IsNotFound(err) {
				resource := &apiv1alpha1.Component{
					ObjectMeta: metav1.ObjectMeta{
						Name:      resourceName,
						Namespace: namespace,
					},
					Spec: apiv1alpha1.ComponentSpec{
						Domain:  domainId,
						Server:  serverId,
						Version: version,
						Image:   image,
					},
				}
				Expect(k8sClient.Create(ctx, resource)).To(Succeed())
			}
		})

		AfterEach(func() {
			// TODO(user): Cleanup logic after each test, like removing the resource instance.
			resource := &apiv1alpha1.Component{}
			err := k8sClient.Get(ctx, typeNamespacedName, resource)
			Expect(err).NotTo(HaveOccurred())

			By("Cleanup the specific resource instance Component")
			Expect(k8sClient.Delete(ctx, resource)).To(Succeed())
		})

		It("should successfully reconcile the resource", func() {
			By("Reconciling the created resource")
			controllerReconciler := &ComponentReconciler{
				Client: k8sClient,
				Scheme: k8sClient.Scheme(),
			}

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})

			Expect(err).NotTo(HaveOccurred())

			// Verify status conditions
			err = k8sClient.Get(ctx, typeNamespacedName, component)
			Expect(err).NotTo(HaveOccurred())
			Expect(component.Status.Conditions).To(HaveLen(1))
			condition := component.Status.Conditions[0]
			Expect(condition).To(Equal(metav1.Condition{
				Type:               "Available",
				Status:             metav1.ConditionTrue,
				Reason:             "Reconciled",
				Message:            "Successfully reconciled component",
				LastTransitionTime: condition.LastTransitionTime, // non-deterministic
			}))

			deployments := &appsv1.DeploymentList{}
			err = k8sClient.List(ctx, deployments)
			Expect(err).To(BeNil(), "Expected Deployment listing to succeed")
			Expect(len(deployments.Items)).To(Equal(1), "Expected a single Deployment to be created")
			deployment := deployments.Items[0]
			Expect(deployment.ObjectMeta.Name).To(Equal("d-0c926b460f60a71c433dc53f2f30380071982d6d845693c2180f16af"))
			Expect(deployment.ObjectMeta.Namespace).To(Equal(namespace))
			Expect(deployment.ObjectMeta.Labels).To(Equal(labels))
			Expect(deployment.Spec).To(Equal(appsv1.DeploymentSpec{
				Selector: &metav1.LabelSelector{
					MatchLabels: labels,
				},
				Template: corev1.PodTemplateSpec{
					ObjectMeta: metav1.ObjectMeta{
						Labels: labels,
					},
					Spec: corev1.PodSpec{
						Containers: []corev1.Container{
							{
								Name:            "grpc",
								Image:           image,
								ImagePullPolicy: "Always",
								// The following are defaults set by K8s.
								// TODO: Make sure these make sense.
								TerminationMessagePath:   "/dev/termination-log",
								TerminationMessagePolicy: "File",
							},
						},
						RuntimeClassName: ptr.To("vimana-runtime"),
						// The following are defaults set by K8s.
						// TODO: Make sure these make sense.
						RestartPolicy:                 "Always",
						TerminationGracePeriodSeconds: ptr.To(int64(30)),
						DNSPolicy:                     "ClusterFirst",
						SecurityContext:               &corev1.PodSecurityContext{},
						SchedulerName:                 "default-scheduler",
					},
				},
				// The following are defaults set by K8s.
				// TODO: Make sure these make sense.
				Replicas: ptr.To(int32(1)),
				Strategy: appsv1.DeploymentStrategy{
					Type: appsv1.RollingUpdateDeploymentStrategyType,
					RollingUpdate: &appsv1.RollingUpdateDeployment{
						MaxUnavailable: ptr.To(intstr.FromString("25%")),
						MaxSurge:       ptr.To(intstr.FromString("25%")),
					},
				},
				RevisionHistoryLimit:    ptr.To(int32(10)),
				ProgressDeadlineSeconds: ptr.To(int32(600)),
			}))

			services := &corev1.ServiceList{}
			err = k8sClient.List(ctx, services)
			Expect(err).To(BeNil(), "Expected Service listing to succeed")
			// There should be 2 services that exist:
			// the one we just created,
			// as well as the K8s API service itself, called "default/kubernetes", which should always exist.
			Expect(len(services.Items)).To(Equal(2), "Expected a single Service to have be created")
			service := getFirstNonK8sService(services)
			Expect(service.ObjectMeta.Name).To(Equal("s-0c926b460f60a71c433dc53f2f30380071982d6d845693c2180f16af"))
			Expect(service.ObjectMeta.Namespace).To(Equal(namespace))
			Expect(service.ObjectMeta.Labels).To(Equal(labels))
			Expect(service.Spec).To(Equal(corev1.ServiceSpec{
				Ports: []corev1.ServicePort{
					{
						Name:        "",
						AppProtocol: ptr.To("kubernetes.io/h2c"),
						Port:        80,
						// The following are defaults set by K8s.
						// TODO: Make sure these make sense.
						Protocol:   "TCP",
						TargetPort: intstr.FromInt32(80),
						NodePort:   0,
					},
				},
				Selector: labels,
				// The following are defaults set by K8s.
				// TODO: Make sure these make sense.
				Type:                  "ClusterIP",
				ClusterIP:             service.Spec.ClusterIP,  // non-deterministic
				ClusterIPs:            service.Spec.ClusterIPs, // non-deterministic
				SessionAffinity:       "None",
				IPFamilies:            []corev1.IPFamily{"IPv4"},
				IPFamilyPolicy:        ptr.To(corev1.IPFamilyPolicy("SingleStack")),
				InternalTrafficPolicy: ptr.To(corev1.ServiceInternalTrafficPolicy("Cluster")),
			}))
		})
	})
})

func getFirstNonK8sService(services *corev1.ServiceList) *corev1.Service {
	for _, service := range services.Items {
		if service.Namespace == "default" && service.Name == "kubernetes" {
			continue
		}
		return &service
	}
	Fail("Expected there to be at least 1 non-Kubernetes service")
	return nil
}
