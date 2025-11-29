use server::exports::adder_service::Guest;
use server::foo::proto::types::{AddFloatsRequest, AddFloatsResponse};
use server::vimana::grpc::types::Context;

struct AdderImpl;

impl Guest for AdderImpl {
    fn add_floats(request: AddFloatsRequest, _context: Context) -> AddFloatsResponse {
        AddFloatsResponse {
            result: request.x + request.y,
        }
    }
}

server::export!(AdderImpl with_types_in server);
