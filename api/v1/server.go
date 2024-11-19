package main

import (
	"fmt"
	"log"
	"net"

	"google.golang.org/grpc"
	"google.golang.org/grpc/reflection"

	"api.vimana.host/v1"
	"api.vimana.host/v1/domains"
)

// Always listen on TCP port 443.
const network = "tcp"
const port = 80

func main() {
	listener, err := net.Listen(network, fmt.Sprintf(":%d", port))
	if err != nil {
		log.Fatalf("Failed to bind to port %d: %v\n", port, err)
	}

	service := v1.NewApiService()
	server := grpc.NewServer()
	domains.RegisterDomainsServer(server, service)
	reflection.Register(server)
	server.Serve(listener)
}
