package main

import (
	"context"
	"fmt"
	"log"
	"net"
	"os"

	"go.uber.org/zap"

	"google.golang.org/grpc"
	"google.golang.org/grpc/reflection"

	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"

	v1 "api.vimana.host/v1"
	"api.vimana.host/v1/domains"
)

// Always listen on TCP port 80.
const network = "tcp"
const port = 80

func main() {
	// Listen for API traffic on TCP port 80 (cleartext).
	// TLS is handled transparently by Ztunnel.
	listener, err := net.Listen(network, fmt.Sprintf(":%d", port))
	if err != nil {
		log.Fatalf("Failed to bind to port %d: %v\n", port, err)
	}

	// Configure the K8s API client for in-cluster access.
	config, err := rest.InClusterConfig()
	if err != nil {
		log.Fatalf("Failed to configure the in-cluster K8s client: %v\n", err)
	}
	client, err := kubernetes.NewForConfig(config)
	if err != nil {
		log.Fatalf("Failed to create K8s client set: %v\n", err)
	}

	// Structured logger used in actions.
	logger, err := zap.NewProduction()
	if err != nil {
		log.Fatalf("Failed to create structured logger: %v\n", err)
	}

	// All K8s API calls are scoped to an explicit namespace.
	namespace := os.Getenv("VIMANA_NAMESPACE")
	if namespace == "" {
		log.Fatalf("Expected the K8s namespace to be explicitly provided.", err)
	}

	service := v1.NewApiService(client, namespace, logger)
	server := grpc.NewServer(grpc.UnaryInterceptor(loggingInterceptor))
	domains.RegisterDomainsServer(server, service)
	reflection.Register(server)
	server.Serve(listener)
}

func loggingInterceptor(
	ctx context.Context,
	request any,
	info *grpc.UnaryServerInfo,
	handler grpc.UnaryHandler,
) (any, error) {
	service, ok := info.Server.(*v1.ApiService)
	if ok {
		service.Logger.Info(
			"Action",
			zap.String("method", info.FullMethod),
		)
	} else {
		log.Printf("Unexpected service type! Got %T\n", info.Server)
	}
	return handler(ctx, request)
}
