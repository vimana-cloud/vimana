package v1

import (
	"k8s.io/client-go/kubernetes"
)

type ApiService struct {
	k8s *kubernetes.Clientset
}

func NewApiService(k8s *kubernetes.Clientset) *ApiService {
	return &ApiService{
		k8s,
	}
}
