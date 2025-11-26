use adder_service::{AddFloatsRequest, AddFloatsResponse, Context, Guest};

struct AdderImpl;

impl Guest for AdderImpl {
    fn add_floats(_ctx: Context, request: AddFloatsRequest) -> AddFloatsResponse {
        AddFloatsResponse {
            result: request.x + request.y,
        }
    }
}

adder_service::export!(AdderImpl with_types_in adder_service);
