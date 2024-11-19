package v1

import (
	"context"
	"log"

	pb "api.vimana.host/v1/domains"
)

func (s *ApiService) Create(ctx context.Context, request *pb.CreateRequest) (*pb.CreateResponse, error) {
	log.Println("Called CREATE")
	return nil, nil
}

func (s *ApiService) List(ctx context.Context, request *pb.ListRequest) (*pb.ListResponse, error) {
	return nil, nil
}

func (s *ApiService) Get(ctx context.Context, request *pb.GetRequest) (*pb.Domain, error) {
	log.Println("Called GET")
	// Currently just echoes the name for testing.
	return &pb.Domain{
		Name: request.Name,
	}, nil
}

func (s *ApiService) UpdateAliases(ctx context.Context, request *pb.UpdateAliasesRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) UpdateOwners(ctx context.Context, request *pb.UpdateOwnersRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) Delete(ctx context.Context, request *pb.DeleteRequest) (*pb.DeleteResponse, error) {
	return nil, nil
}
