package v1

import (
	"go.uber.org/zap"

	"k8s.io/client-go/kubernetes"
)

type ApiService struct {
	k8s       *kubernetes.Clientset
	namespace string
	Logger    *zap.Logger
}

func NewApiService(k8s *kubernetes.Clientset, namespace string, logger *zap.Logger) *ApiService {
	return &ApiService{k8s, namespace, logger}
}
