#[allow(unused_imports)]
use std::net::TcpListener;
use std::{
    env, fs,
    io::{Read, Write},
    net::TcpStream,
    path::Path,
};
use tokio;

const CLRF: &str = "\r\n";

#[derive(PartialEq, Debug)]
enum HttpMethod {
    GET,
    POST,
    PUT,
    DELETE,
}

#[derive(PartialEq, Debug)]
enum HttpVersion {
    V1_0,
    V1_1,
}

#[derive(PartialEq, Debug)]
enum HttpResponseHeaders {
    ContentType,
    ContentLength,
}

impl HttpResponseHeaders {
    fn as_str(&self) -> &'static str {
        match self {
            HttpResponseHeaders::ContentType => "Content-Type: ",
            HttpResponseHeaders::ContentLength => "Content-Length: ",
        }
    }
}

struct HttpResponseHeaderBuilder {
    response_text: String,
}

impl HttpResponseHeaderBuilder {
    pub fn new() -> Self {
        HttpResponseHeaderBuilder {
            response_text: String::new(),
        }
    }

    pub fn add(self, response_header: HttpResponseHeaders, value: String) -> Self {
        HttpResponseHeaderBuilder {
            response_text: self.response_text
                + response_header.as_str()
                + &value
                + &CLRF.to_string(),
        }
    }

    pub fn get_response_string(self) -> String {
        self.response_text
    }
}

#[derive(PartialEq, Debug)]
enum HttpRequestHeaders {
    UserAgent,
    Host,
    Accept,
    ContentType,
    ContentLength,
}

impl HttpRequestHeaders {
    fn as_str(&self) -> &'static str {
        match self {
            HttpRequestHeaders::UserAgent => "User-Agent",
            HttpRequestHeaders::Host => "Host",
            HttpRequestHeaders::Accept => "Accept",
            HttpRequestHeaders::ContentType => "Content-Type",
            HttpRequestHeaders::ContentLength => "Content-Length",
        }
    }

    fn from_str(header: &str) -> Option<Self> {
        match header.to_lowercase().as_str() {
            // headers are case insensitive
            "user-agent" => Some(HttpRequestHeaders::UserAgent),
            "host" => Some(HttpRequestHeaders::Host),
            "accept" => Some(HttpRequestHeaders::Accept),
            "content-type" => Some(HttpRequestHeaders::ContentType),
            "content-length" => Some(HttpRequestHeaders::ContentLength),
            _ => None,
        }
    }
}

#[derive(PartialEq, Debug)]
enum ContentType {
    TextPlain,
    ApplicationOctetStream,
    Other(String),
}

impl ContentType {
    fn from_str(content_type: &str) -> Self {
        match content_type.to_lowercase().as_str() {
            "text/plain" => ContentType::TextPlain,
            "application/octet-stream" => ContentType::ApplicationOctetStream,
            other => ContentType::Other(other.to_string()),
        }
    }

    fn as_str(&self) -> String {
        match self {
            ContentType::TextPlain => "text/plain".to_string(),
            ContentType::ApplicationOctetStream => "application/octet-stream".to_string(),
            ContentType::Other(s) => s.clone(),
        }
    }
}

struct HttpRequestHeaderParser {
    headers: Vec<(HttpRequestHeaders, String)>,
}

impl HttpRequestHeaderParser {
    pub fn new() -> Self {
        HttpRequestHeaderParser {
            headers: Vec::new(),
        }
    }

    pub fn parse(&mut self, header_lines: &[&str]) {
        for line in header_lines {
            if let Some((key, value)) = line.split_once(": ") {
                if let Some(header) = HttpRequestHeaders::from_str(key) {
                    self.headers.push((header, value.trim().to_string()));
                }
            }
        }
    }

    pub fn get_header_value(&self, header: HttpRequestHeaders) -> Option<&String> {
        self.headers
            .iter()
            .find(|(h, _)| h == &header)
            .map(|(_, v)| v)
    }

    pub fn get_content_type(&self) -> Option<ContentType> {
        self.get_header_value(HttpRequestHeaders::ContentType)
            .map(|ct| ContentType::from_str(ct))
    }

    pub fn is_content_type(&self, content_type: ContentType) -> bool {
        self.get_content_type()
            .map_or(false, |ct| ct == content_type)
    }
}

struct HttpRequestBody {
    raw_content: Vec<u8>,
}

impl HttpRequestBody {
    pub fn new(raw_content: Vec<u8>) -> Self {
        HttpRequestBody { raw_content }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.raw_content
    }

    pub fn as_string(&self) -> Option<String> {
        String::from_utf8(self.raw_content.clone()).ok()
    }
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut _stream) => {
                tokio::spawn(async move { handle_connection(_stream).await });
            }
            Err(e) => {
                println!("server error: {}", e);
            }
        }
    }
}

async fn handle_connection(mut stream: TcpStream) {
    let mut raw_request: [u8; 1024] = [0; 1024]; // the request is assumed to be less than 1024 bytes
    stream.read(&mut raw_request).unwrap();

    // Split the raw request into headers and body sections
    let raw_request_str = String::from_utf8_lossy(&raw_request);
    let parts: Vec<&str> = raw_request_str.split(&CLRF.repeat(2)).collect(); // two CLRF characters seems to seperate the request line+headers and body

    // Parse headers section
    let header_section = parts[0];
    let mut lines = header_section.lines();
    let request_line = lines.next().unwrap_or("");
    let headers = lines.collect::<Vec<&str>>();

    let (method, path, version) = parse_request_line(request_line).unwrap();

    if version != HttpVersion::V1_1 {
        respond_bad_request(
            stream,
            None,
            Some("this server only supports HTTP version 1.1.".to_string()),
        );
        return;
    }

    let mut header_parser = HttpRequestHeaderParser::new();
    header_parser.parse(&headers);

    // Parse body if it exists
    let body = if parts.len() > 1 {
        let body_content = parts[1].as_bytes().to_vec();
        Some(HttpRequestBody::new(body_content))
    } else {
        None
    };

    match method {
        HttpMethod::GET if path == "/" => respond_ok(stream, None, None),
        HttpMethod::GET if path.starts_with("/echo/") => endpoint_get_echo(stream, path),
        HttpMethod::GET if path == "/user-agent" => endpoint_get_user_agent(stream, &header_parser),
        HttpMethod::GET if path.starts_with("/files/") => endpoint_get_files(stream, path),
        HttpMethod::POST if path.starts_with("/files/") => {
            endpoint_post_files(stream, path, &header_parser, body)
        }
        _ => respond_not_found(stream, None, None),
    }
}

fn parse_request_line(request_line: &str) -> Option<(HttpMethod, &str, HttpVersion)> {
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() == 3 {
        let method = match parts[0] {
            "GET" => Some(HttpMethod::GET),
            "POST" => Some(HttpMethod::POST),
            "PUT" => Some(HttpMethod::PUT),
            "DELETE" => Some(HttpMethod::DELETE),
            _ => None,
        }?;

        let version = match parts[2] {
            "HTTP/1.0" => Some(HttpVersion::V1_0),
            "HTTP/1.1" => Some(HttpVersion::V1_1),
            _ => None,
        }?;

        // parts[1] is the path that is being requested
        return Some((method, parts[1], version));
    }
    None
}

fn endpoint_get_echo(stream: TcpStream, path: &str) {
    let resp_value: String = path
        .split("/")
        .skip(2) // skip the inital path / and the echo/ portion as well
        .collect::<Vec<&str>>()
        .join("/"); // last join incase the string has more "/" chars in it
    respond_string_body(stream, resp_value, None);
}

fn endpoint_get_user_agent(stream: TcpStream, header_parser: &HttpRequestHeaderParser) {
    if let Some(user_agent) = header_parser.get_header_value(HttpRequestHeaders::UserAgent) {
        respond_string_body(stream, user_agent.to_string(), None);
    } else {
        respond_not_found(stream, None, None);
    }
}

fn endpoint_get_files(stream: TcpStream, path: &str) {
    let file_name: String = path
        .split("/")
        .skip(2) // skip the inital path / and the files/ portion as well
        .collect::<Vec<&str>>()
        .join("/"); // last join incase this is a path with sub-directories
    let env_args: Vec<String> = env::args().collect();
    let mut file_path = env_args[2].clone();
    file_path.push_str(&file_name);

    if let Ok(mut file) = fs::File::open(Path::new(&file_path)) {
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        respond_byte_body(stream, buf, Some("application/octet-stream".to_string()));
    } else {
        respond_not_found(stream, None, None);
    }
}

fn endpoint_post_files(
    stream: TcpStream,
    path: &str,
    headers: &HttpRequestHeaderParser,
    body: Option<HttpRequestBody>,
) {
    let file_name: String = path
        .split("/")
        .skip(2) // skip the inital path / and the files/ portion as well
        .collect::<Vec<&str>>()
        .join("/"); // last join incase this is a path with sub-directories
    let env_args: Vec<String> = env::args().collect();
    let mut file_path = env_args[2].clone();
    file_path.push_str(&file_name);

    if !headers.is_content_type(ContentType::ApplicationOctetStream) {
        respond_bad_request(stream, None, Some("unexpected content type".to_string()));
        return;
    }

    if let Ok(_) = fs::metadata(&file_path) {
        respond_conflict(stream, None, Some("file already exists.".to_string()));
        return;
    }

    match body {
        Some(body) => {
            if let Ok(mut file) = fs::File::create_new(Path::new(&file_path)) {
                match file.write_all(body.as_bytes()) {
                    Ok(_) => respond_created(stream, None, None),
                    Err(_) => respond_internal_server_error(
                        stream,
                        None,
                        Some("failed to write to file.".to_string()),
                    ),
                }
            } else {
                respond_internal_server_error(
                    stream,
                    None,
                    Some("failed to create file.".to_string()),
                );
            }
        }
        None => respond_bad_request(stream, None, Some("No body provided".to_string())),
    }
}

fn respond_string_body(stream: TcpStream, body: String, content_type: Option<String>) {
    let header = HttpResponseHeaderBuilder::new()
        .add(
            HttpResponseHeaders::ContentType,
            content_type.unwrap_or("text/plain".to_string()),
        )
        .add(HttpResponseHeaders::ContentLength, body.len().to_string())
        .get_response_string();

    respond_ok(stream, Some(header), Some(body));
}

fn respond_byte_body(mut stream: TcpStream, body: Vec<u8>, content_type: Option<String>) {
    let header = HttpResponseHeaderBuilder::new()
        .add(
            HttpResponseHeaders::ContentType,
            content_type.unwrap_or("text/plain".to_string()),
        )
        .add(HttpResponseHeaders::ContentLength, body.len().to_string())
        .get_response_string();
    let header_buf = format!("HTTP/1.1 {} {}\r\n{}\r\n", 200, "OK", header,);
    stream.write_all(header_buf.as_bytes()).unwrap();
    stream.write_all(&body).unwrap();
}

fn respond_ok(mut stream: TcpStream, headers: Option<String>, body: Option<String>) {
    let buf = format!(
        "HTTP/1.1 {} {}\r\n{}\r\n{}",
        200,
        "OK",
        headers.unwrap_or_default(),
        body.unwrap_or_default()
    );
    stream.write(&buf.as_bytes()).unwrap();
}

fn respond_not_found(mut stream: TcpStream, headers: Option<String>, body: Option<String>) {
    let buf = format!(
        "HTTP/1.1 {} {}\r\n{}\r\n{}",
        404,
        "Not Found",
        headers.unwrap_or_default(),
        body.unwrap_or_default()
    );
    stream.write(&buf.as_bytes()).unwrap();
}

fn respond_created(mut stream: TcpStream, headers: Option<String>, body: Option<String>) {
    let buf = format!(
        "HTTP/1.1 {} {}\r\n{}\r\n{}",
        201,
        "Created",
        headers.unwrap_or_default(),
        body.unwrap_or_default()
    );
    stream.write(&buf.as_bytes()).unwrap();
}

fn respond_conflict(mut stream: TcpStream, headers: Option<String>, body: Option<String>) {
    let buf = format!(
        "HTTP/1.1 {} {}\r\n{}\r\n{}",
        409,
        "Conflict",
        headers.unwrap_or_default(),
        body.unwrap_or_default()
    );
    stream.write(&buf.as_bytes()).unwrap();
}

fn respond_internal_server_error(
    mut stream: TcpStream,
    headers: Option<String>,
    body: Option<String>,
) {
    let buf = format!(
        "HTTP/1.1 {} {}\r\n{}\r\n{}",
        500,
        "Internal Server Error",
        headers.unwrap_or_default(),
        body.unwrap_or_default()
    );
    stream.write(&buf.as_bytes()).unwrap();
}

fn respond_bad_request(mut stream: TcpStream, headers: Option<String>, body: Option<String>) {
    let buf = format!(
        "HTTP/1.1 {} {}\r\n{}\r\n{}",
        400,
        "Bad Request",
        headers.unwrap_or_default(),
        body.unwrap_or_default()
    );
    stream.write(&buf.as_bytes()).unwrap();
}
