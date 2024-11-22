package v1

import (
	"context"
	"fmt"

	pb "api.vimana.host/v1/domains"
)

func (s *ApiService) Create(ctx context.Context, request *pb.CreateDomainRequest) (*pb.CreateDomainResponse, error) {
	networking_rest := s.k8s.NetworkingV1().RESTClient()
	gateway, err := networking_rest.Get().Namespace(s.namespace).Do(ctx).Get()
	s.Logger.Info(fmt.Sprintf("Tried: %v, %v", gateway, err))
	return nil, nil
}

func (s *ApiService) List(ctx context.Context, request *pb.ListDomainsRequest) (*pb.ListDomainsResponse, error) {
	return nil, nil
}

func (s *ApiService) Get(ctx context.Context, request *pb.GetDomainRequest) (*pb.Domain, error) {
	// Currently just echoes the name for testing.
	return &pb.Domain{
		Name: request.Name,
	}, nil
}

func (s *ApiService) UpdateAliases(ctx context.Context, request *pb.UpdateDomainAliasesRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) UpdateOwners(ctx context.Context, request *pb.UpdateDomainOwnersRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) Delete(ctx context.Context, request *pb.DeleteDomainRequest) (*pb.DeleteDomainResponse, error) {
	return nil, nil
}
