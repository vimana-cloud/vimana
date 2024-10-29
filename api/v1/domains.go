package v1

import (
	"context"

	pb "api.vimana.host/v1/domains"
)

func (s *ApiService) Create(ctx context.Context, point *pb.CreateRequest) (*pb.CreateResponse, error) {
	return nil, nil
}

func (s *ApiService) List(ctx context.Context, point *pb.ListRequest) (*pb.ListResponse, error) {
	return nil, nil
}

func (s *ApiService) Get(ctx context.Context, point *pb.GetRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) UpdateAliases(ctx context.Context, point *pb.UpdateAliasesRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) UpdateOwners(ctx context.Context, point *pb.UpdateOwnersRequest) (*pb.Domain, error) {
	return nil, nil
}

func (s *ApiService) Delete(ctx context.Context, point *pb.DeleteRequest) (*pb.DeleteResponse, error) {
	return nil, nil
}