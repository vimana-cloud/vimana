package main

import (
	"crypto/tls"
	"fmt"
	"log"
	"net"
	"os"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/reflection"

	"api.vimana.host/v1"
	"api.vimana.host/v1/domains"
)

// Always listen on TCP port 443.
const network = "tcp"
const port = 443

// Environment variable from which to load TLS credentials.
const tlsCertKey = "TLS_CERT"
const tlsKeyKey = "TLS_KEY"

func main() {
	tlsCert := os.Getenv(tlsCertKey)
	tlsKey := os.Getenv(tlsKeyKey)
	certificate, err := tls.X509KeyPair([]byte(tlsCert), []byte(tlsKey))
	if err != nil {
		// Log the loaded certificate for debugging,
		// but never log the private key.
		log.Fatalf("Failed to load TLS credentials from environment variables %s and %s.\nCertificate:%q\n", tlsCertKey, tlsKeyKey, tlsCert)
	}

	listener, err := net.Listen(network, fmt.Sprintf(":%d", port))
	if err != nil {
	  log.Fatalf("Failed to bind to port %d: %v\n", port, err)
	}

	service := v1.NewApiService()
	server := grpc.NewServer(grpc.Creds(credentials.NewServerTLSFromCert(&certificate)))
	domains.RegisterDomainsServer(server, service)
	reflection.Register(server)
	server.Serve(listener)
}
