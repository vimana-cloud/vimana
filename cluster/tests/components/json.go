package main

import (
	"fmt"

	jsontranscodingservice "bar/proto/server/transcoding/proto/server/json-transcoding-service"
	grpc "bar/proto/server/vimana/grpc/types"

	"go.bytecodealliance.org/cm"
)

func init() {
	jsontranscodingservice.Exports.Echo = func(
		request jsontranscodingservice.EchoRequest,
		context jsontranscodingservice.Context,
	) cm.Result[
		jsontranscodingservice.StatusShape,
		jsontranscodingservice.EchoResponse,
		grpc.Status,
	] {
		return cm.OK[cm.Result[
			jsontranscodingservice.StatusShape,
			jsontranscodingservice.EchoResponse,
			grpc.Status,
		]](jsontranscodingservice.EchoResponse{
			Message: fmt.Sprintf("%s %s", request.Message, request.Message),
		})
	}
}

func main() {}
