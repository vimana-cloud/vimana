use std::fs::File;

use server::bar::proto::types::{Empty, StringRequest, U64Request};
use server::exports::wasi_exercise::Guest;
use server::vimana::grpc::types::{Context, Status, StatusCode};
use server::wasi::clocks::wall_clock::now;
use server::wasi::http::outgoing_handler::handle as http_handle;
use server::wasi::http::types::{Headers, OutgoingRequest, Scheme};
use server::wasi::io::poll::poll;
use url::Url;

struct WasiExercise;
server::export!(WasiExercise);

impl Guest for WasiExercise {
    fn assert_no_filesystem(request: StringRequest, _context: Context) -> Result<Empty, Status> {
        result_fail(if File::open(&request.input).is_ok() {
            "File can be opened via stdlib"
        } else if File::create(&request.input).is_ok() {
            "File can be created via stdlib"
        } else {
            return result_pass();
        })
    }

    fn assert_internet_access(request: StringRequest, _context: Context) -> Result<Empty, Status> {
        result_fail(if let Ok(url) = Url::parse(&request.input) {
            let http_request = OutgoingRequest::new(Headers::new());
            let scheme = match url.scheme() {
                "http" => Scheme::Http,
                "https" => Scheme::Https,
                scheme => Scheme::Other(String::from(scheme)),
            };
            if let Ok(_) = http_request.set_scheme(Some(&scheme)) {
                if let Ok(_) = http_request.set_authority(url.host_str()) {
                    if let Ok(_) = http_request.set_path_with_query(Some(url.path())) {
                        match http_handle(http_request, None) {
                            Ok(future_response) => {
                                // Wait for the response to be ready.
                                let pollable = future_response.subscribe();
                                poll(&[&pollable]);
                                match future_response.get() {
                                    Some(Ok(Ok(_response))) => return result_pass(),
                                    Some(Ok(Err(error))) => {
                                        format!("Got error response: {:?}", error)
                                    }
                                    Some(Err(())) => String::from("Response already consumed"),
                                    None => String::from("Response not ready after poll"),
                                }
                            }
                            Err(error) => format!("Failed invoking http.handle: {:?}", error),
                        }
                    } else {
                        String::from("Failed setting request path with query")
                    }
                } else {
                    String::from("Failed setting request authority")
                }
            } else {
                String::from("Failed setting request scheme")
            }
        } else {
            String::from("Cannot parse URL")
        })
    }

    fn assert_clock_access(request: U64Request, _context: Context) -> Result<Empty, Status> {
        let seconds = now().seconds;
        const DELTA: u64 = 3;
        result_fail(
            if seconds + DELTA > request.input && seconds - DELTA < request.input {
                return result_pass();
            } else {
                format!("Wrong time: got {} but wanted {}", seconds, request.input)
            },
        )
    }
}

fn result_pass() -> Result<Empty, Status> {
    Ok(Empty { empty: None })
}

fn result_fail<S: Into<String>>(message: S) -> Result<Empty, Status> {
    Err(Status {
        code: StatusCode::Internal,
        message: message.into(),
    })
}
