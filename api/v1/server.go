package main

import (
	"fmt"
	"log"
	"net"

	"google.golang.org/grpc"
	"google.golang.org/grpc/reflection"

	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"

	"api.vimana.host/v1/domains"
	v1 "api.vimana.host/v1"
)

// Always listen on TCP port 443.
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

	service := v1.NewApiService(client)
	server := grpc.NewServer()
	domains.RegisterDomainsServer(server, service)
	reflection.Register(server)
	server.Serve(listener)
}
