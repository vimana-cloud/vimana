package main

import (
	"fmt"

	jsontranscodingservice "bar/proto/server/transcoding/proto/server/json-transcoding-service"
)

func init() {
	jsontranscodingservice.Exports.Echo = func(
		request jsontranscodingservice.EchoRequest,
		context jsontranscodingservice.Context,
	) jsontranscodingservice.EchoResponse {
		return jsontranscodingservice.EchoResponse{Message: fmt.Sprintf("%s %s", request.Message, request.Message)}
	}
}

func main() {}
