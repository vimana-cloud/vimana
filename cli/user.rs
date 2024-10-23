//! User subcommands for the Vimana CLI.

use std::collections::HashMap;
use std::io::{stdin, BufRead, BufReader, Error as IoError, ErrorKind as IoErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, ExitCode};
use std::thread::{spawn, JoinHandle};
use std::time::SystemTime;

use httpdate::fmt_http_date;
use reqwest::blocking::Client as HttpClient;
use reqwest::Url;

// Status:
// Played around with this, and some of this logic may be useful.
// Unfortunately, we need to handle more of this server-side
// in order to protect the OAuth apps' client secrets.
//
// Here's how it almost looks now:
//
// 1. The CLI goes to Vimana's login page, where user selects ID provider.
// 2. Vimana redirects to GitHub's authorization endpoint (giving client ID).
// 3. GitHub redirects to localhost, (giving authz code).
// 4. The CLI goes to the token endpoint (giving both client ID AND SECRET).
// 5. GitHub redirects to localhost again, (giving ID token).
//
// And here's how it should look:
//
// 1. The CLI goes to Vimana's login page, where user selects ID provider.
// 2. Vimana redirects to GitHub's authorization endpoint (giving client ID).
// 3. GitHub redirects to Vimana, (giving authz code).
// 4. Vimana goes to the token endpoint (giving both client ID AND SECRET).
// 5. GitHub redirects to Vimana again, (giving ID token).
// 6. Vimana redirects to localhost (giving ID token).

/// Login URI used when we're able to bind to the local callback port.
// TODO: Use the proper login URL when it's available.
//       Default to GitHub-only for now.
//const LOGIN_URI_AUTO: &str = "https://vimana.host/login?cli=auto";
// TODO: Use the `state` parameter in GitHub's authorization endpoint to improve security.
const LOGIN_URI_AUTO: &str = "https://github.com/login/oauth/authorize?client_id=Ov23lijpkaQ4ChTLTfAU&redirect_uri=http%3A%2F%2F127.0.0.1%3A61803&scope=user%3aemail%20email";
/// Login URI used when we cannot bind to port 61803.
/// The user must copy and paste the ID token manually.
const LOGIN_URI_MANUAL: &str = "https://vimana.host/login?cli=manual";

const CLIENT_ID: &str = "Ov23lijpkaQ4ChTLTfAU";
const CLIENT_SECRET: &str = "fake";

// TODO: See if we can get away with a dynamic port number.
const LOCAL_CALLBACK_PORT: u16 = 61803;
// TODO: Use something like `concat!()`
//       if it can support constants instead of just literals.
const LOCAL_REDIRECT_ADDRESS: &str = "127.0.0.1:61803";

/// HTTP query param key for the authorization code in the redirect request.
const AUTHZ_CODE_PARAM_KEY: &str = "code";

/// Execute the `user login` command and return the overall exit code.
pub fn login(manual: bool) -> ExitCode {
    let token = if manual {
        // The `--manual` flag indicates that
        // the user wants to both open the browser manually
        // and copy the ID token manually.
        ask_to_open(LOGIN_URI_MANUAL);
        login_manual()
    } else {
        login_auto()
    };
    match token {
        Ok(token) => {
            // TODO: Cache the token locally.
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error);
            eprintln!("Failed to read the ID token. Try with --manual if issues recur.");
            ExitCode::FAILURE
        }
    }
}

/// Execute the `user logout` command and return the overall exit code.
pub fn logout() -> ExitCode {
    todo!()
}

/// Direct the user to the login URI
/// and try to retrive the ID token automatically from the local redirect port.
/// If listening on that port fails, fall back on manual token input.
fn login_auto() -> Result<Vec<u8>, IoError> {
    match setup_local_callback() {
        Ok(join_handle) => {
            open(LOGIN_URI_AUTO);
            join_handle.join().unwrap_or(Err(IoError::new(
                IoErrorKind::Other,
                "Callback thread panicked",
            )))
        }
        Err(_) => {
            eprintln!(
                "Could not bind to TCP port {LOCAL_CALLBACK_PORT}. Falling back on manual token input."
            );
            open(LOGIN_URI_MANUAL);
            login_manual()
        }
    }
}

/// Assuming that the user has already been directed to the login URI,
/// retrieve the ID token from standard input.
fn login_manual() -> Result<Vec<u8>, IoError> {
    eprint!("\nEnter token: ");
    let mut buffer = String::new();
    stdin().read_line(&mut buffer).map(|_| buffer.into_bytes())
}

/// Try to bind to the automatic callback port. Return immediately.
/// On success, a join handle to the token is returned.
fn setup_local_callback() -> Result<JoinHandle<Result<Vec<u8>, IoError>>, IoError> {
    TcpListener::bind(LOCAL_REDIRECT_ADDRESS)
        .map(|listener| spawn(move || listen_for_callback(listener)))
}

/// Block until a TCP connection is opened on the local redirect port.
/// Read and parse an authorization code from the frist request,
/// return an appropriate HTTP response,
/// then exchange the code for an ID token at the token endpoint.
fn listen_for_callback(listener: TcpListener) -> Result<Vec<u8>, IoError> {
    let (mut stream, _addr) = listener.accept()?;
    let reader = BufReader::new(&stream);
    let mut lines = reader.lines();

    // The start line of an HTTP/1 request has three space-delimited parts:
    //     <verb> <uri> <protocol>
    // This contains all the important parts for our purposes.
    let start_line = lines
        .next()
        .ok_or_else(|| malformed_redirect_error(&mut stream))??;

    let mut start_line = start_line.split(' ');
    let verb = start_line
        .next()
        .ok_or_else(|| malformed_redirect_error(&mut stream))?;
    if verb != "GET" {
        return Err(malformed_redirect_error(&mut stream));
    }
    let url = Url::parse(
        // Give the URL an arbitrary base so it can be parsed.
        &format!(
            "http://localhost{}",
            start_line
                .next()
                .ok_or_else(|| malformed_redirect_error(&mut stream))?
        ),
    )
    .map_err(|_| malformed_redirect_error(&mut stream))?;
    if url.path() == "/favicon.ico" {
        return listen_for_callback(listener);
    }
    let query_params = url.query_pairs();

    // Extract the relevant parameters.
    let mut authz_code: Option<String> = None;
    for (key, value) in query_params {
        if key == AUTHZ_CODE_PARAM_KEY {
            authz_code = Some(String::from(value));
        }
    }

    // TODO: Also validate the `state` key for security.
    let authz_code = authz_code.ok_or_else(|| malformed_redirect_error(&mut stream))?;

    #[allow(unused_must_use)]
    {
        // Best effort to write the success response
        // because we already have the authz code and don't need the browser anymore.
        write!(
            stream,
            concat!(
                "HTTP/1.1 200 OK\n",
                "Content-Type: text/html; charset=utf-8\n",
                "Date: {}\n",
                "\n",
                "<!DOCTYPE html>\n",
                "<html>",
                "<body>",
                "<p>",
                "Success! 🎉 You can close this tab now.",
                "</p>",
                "</body>",
                "</html>",
            ),
            fmt_http_date(SystemTime::now()),
        );
    }

    retrieve_id_token(listener, authz_code)
}

/// Write an HTTP 400 response to the TCP stream
/// and return an error indicating trouble parsing the redirect request.
fn malformed_redirect_error(stream: &mut TcpStream) -> IoError {
    #[allow(unused_must_use)]
    {
        // Best effort to write the failure response
        // because we already have an error to return.
        write!(
            stream,
            concat!(
                "HTTP/1.1 400 Bad Request\n",
                "Content-Type: text/html; charset=utf-8\n",
                "Date: {}\n",
                "\n",
                "<!DOCTYPE html>\n",
                "<html>",
                "<body>",
                "<p>",
                "Whoops! 😞 Something went wrong with the OAuth callback request.",
                "</p>",
                "<p>",
                "Check the CLI for more information.",
                "</p>",
                "</body>",
                "</html>",
            ),
            fmt_http_date(SystemTime::now()),
        );
    }
    IoError::new(IoErrorKind::InvalidInput, "Malformed redirect request")
}

/// Given a code from the authorization endpoint
/// (step 1 of the OIDC Authorization Code Flow),
/// Retrieve an ID token from the token endpoint (step 2).
fn retrieve_id_token(listener: TcpListener, authz_code: String) -> Result<Vec<u8>, IoError> {
    let client = HttpClient::new();
    let mut params = HashMap::with_capacity(5);
    eprintln!("Exchanging for code: {}", &authz_code);
    params.insert(AUTHZ_CODE_PARAM_KEY, authz_code);
    params.insert("client_id", String::from(CLIENT_ID));
    params.insert("client_secret", String::from(CLIENT_SECRET));
    params.insert("redirect_uri", String::from("http://127.0.0.1:61803"));
    // TODO: I guess most of this should have happened server-side.
    let response = client
        .post("https://github.com/login/oauth/access_token")
        .json(&params)
        .send()
        .unwrap();
    eprintln!("{}", response.text().unwrap());
    Ok(Vec::new())
}

/// Default command to open a URL on Linux.
#[cfg(target_os = "linux")]
const DEFAULT_OPEN_CMD: &str = "xdg-open";

/// Default command to open a URL on MacOS.
#[cfg(target_os = "macos")]
const DEFAULT_OPEN_CMD: &str = "open";

/// On any supported platform with a default open command,
/// try to open the web browser for the user.
/// Return `true` iff we think the browser opened successfully.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open(url: &str) {
    eprintln!("Opening a web browser to complete login...");
    if let Ok(status) = Command::new(DEFAULT_OPEN_CMD).arg(url).status() {
        if status.success() {
            return;
        }
    }
    eprintln!("Failed to open a browser.");
    ask_to_open(url)
}

/// On platforms where the default open command is non-obvious,
/// or when opening the browser automatically fails,
/// fall back to simply asking the user to open their web browser.
/// Always returns `false`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn open(url: &str) {
    ask_to_open(url)
}

/// Ask the user to open their browser at the login URL.
/// Always returns `false`.
fn ask_to_open(url: &str) {
    eprintln!("\nVisit this URL to log in: {url}")
}
