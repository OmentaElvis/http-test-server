//! # HTTP Test Server
//!
//! Programatically create end-points that listen for connections and return pre-defined
//! responses.
//!
//! - Allows multiple endpoints and simultaneous client connections
//! - Streaming support
//! - Helper functions to retrieve data such as request count, number of connected clients and
//! requests metadata
//! - Automatically allocates free port and close server after use
//!
//! # Examples:
//!
//! Accept POST requests:
//! ```
//! extern crate http_test_server;
//!
//! use http_test_server::{TestServer, Resource};
//! use http_test_server::http::{Status, Method};
//!
//! let server = TestServer::new().unwrap();
//! let resource = server.create_resource("/some-endpoint/new");
//!
//! resource
//!     .status(Status::Created)
//!     .method(Method::POST)
//!     .header("Content-Type", "application/json")
//!     .header("Cache-Control", "no-cache")
//!     .body("{ \"message\": \"this is a message\" }");
//!
//! // request: POST /some-endpoint/new
//!
//! // HTTP/1.1 201 Created\r\n
//! // Content-Type: application/json\r\n
//! // Cache-Control: no-cache\r\n
//! // \r\n
//! // { "message": "this is a message" }
//!
//!
//! ```
//!
//! Use path and query parameters:
//! ```
//! extern crate http_test_server;
//!
//! use http_test_server::{TestServer, Resource};
//! use http_test_server::http::{Status, Method};
//!
//! let server = TestServer::new().unwrap();
//! let resource = server.create_resource("/user/{userId}?filter=*");

//! resource
//!     .status(Status::OK)
//!     .header("Content-Type", "application/json")
//!     .header("Cache-Control", "no-cache")
//!     .body(r#"{ "id": "{path.userId}", "filter": "{query.filter}" }"#);
//!
//! // request: GET /user/abc123?filter=all
//!
//! // HTTP/1.1 200 Ok\r\n
//! // Content-Type: application/json\r\n
//! // Cache-Control: no-cache\r\n
//! // \r\n
//! // { "id": "abc123", "filter": "all" }
//!
//!
//! ```
//!
//! Expose a persistent stream:
//! ```
//! # extern crate http_test_server;
//! # use http_test_server::{TestServer, Resource};
//! # use http_test_server::http::{Status, Method};
//! let server = TestServer::new().unwrap();
//! let resource = server.create_resource("/sub");
//!
//! resource
//!     .header("Content-Type", "text/event-stream")
//!     .header("Cache-Control", "no-cache")
//!     .stream()
//!     .body(": initial data");
//!
//! // ...
//!
//! resource
//!     .send("some data")
//!     .send(" some extra data\n")
//!     .send_line("some extra data with line break")
//!     .close_open_connections();
//!
//! // request: GET /sub
//!
//! // HTTP/1.1 200 Ok\r\n
//! // Content-Type: text/event-stream\r\n
//! // Cache-Control: no-cache\r\n
//! // \r\n
//! // : initial data
//! // some data some extra data\n
//! // some extra data with line break\n
//!
//!
//! ```
//! Redirects:
//! ```
//! # extern crate http_test_server;
//! # use http_test_server::{TestServer, Resource};
//! # use http_test_server::http::{Status, Method};
//! let server = TestServer::new().unwrap();
//! let resource_redirect = server.create_resource("/original");
//! let resource_target = server.create_resource("/new");
//!
//! resource_redirect
//!     .status(Status::SeeOther)
//!     .header("Location", "/new" );
//!
//! resource_target.body("Hi!");
//!
//! // request: GET /original
//!
//! // HTTP/1.1 303 See Other\r\n
//! // Location: /new\r\n
//! // \r\n
//!
//!
//! ```
//! Simple Regex URI:
//!
//! ```
//! # extern crate http_test_server;
//! # use http_test_server::{TestServer, Resource};
//! # use http_test_server::http::{Status, Method};
//! let server = TestServer::new().unwrap();
//! let resource = server.create_resource("/hello/[0-9]/[A-z]/.*");
//!
//! // request: GET /hello/8/b/doesntmatter-hehe
//!
//! // HTTP/1.1 200 Ok\r\n
//! // \r\n
//!
//! ```
//! Complex regex with custom capture groups
//!
//! ```
//! # extern crate http_test_server;
//! # use http_test_server::{TestServer, Resource};
//! # use http_test_server::http::{Status, Method};
//! let server = TestServer::new().unwrap();
//! let resource = server.create_resource_with_regex("/hello/(?<id>[0-9])/[A-z]/.*");
//! resource.body("id: {path.id}");
//!
//! // request: GET /hello/8/b/doesntmatter-hehe
//!
//! // HTTP/1.1 200 Ok\r\n
//! // \r\n
//! // id: 8
//!
//! ```
//!
//! *NOTE*: This is not intended to work as a full featured server. For this reason, many validations
//! and behaviours are not implemented. e.g: A request with `Accept` header with not supported
//! `Content-Type` won't trigger a `406 Not Acceptable`.
//!
//! As this crate was devised to be used in tests, smart behaviours could be confusing and misleading. Having said that, for the sake of convenience, some default behaviours were implemented:
//!
//! - Server returns `404 Not Found` when requested resource was not configured.
//! - Server returns `405 Method Not Allowed` when trying to reach resource with different method from those configured.
//! - When a resource is created it responds to `GET` with `200 Ok` by default.
extern crate regex;

pub mod resource;
pub mod http;

use std::thread;
use std::net::TcpListener;
use std::net::TcpStream;
use std::io::prelude::*;
use std::io::Error;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::collections::HashMap;
use http::Method;
use http::Status;
pub use resource::Resource;
use resource::URIParameters;
use regex::Regex;

type ServerResources = Arc<Mutex<Vec<Resource>>>;
type RequestsTX = Arc<Mutex<Option<mpsc::Sender<Request>>>>;

/// Controls the listener life cycle and creates new resources
pub struct TestServer {
    port: u16,
    resources: ServerResources,
    requests_tx: RequestsTX
}

impl TestServer {
    /// Creates a listener that is bounded to a free port in localhost.
    /// Listener is closed when the value is dropped.
    ///
    /// Any request for non-defined resources will return 404.
    ///
    /// ```
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    /// let server = TestServer::new().unwrap();
    ///
    /// ```
    pub fn new() -> Result<TestServer, Error> {
        TestServer::new_with_port(0)
    }

    /// Same behaviour as `new`, but tries to bound to given port instead of looking for a free one.
    /// ```
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    /// let server = TestServer::new_with_port(8080).unwrap();
    ///
    /// ```
    pub fn new_with_port(port: u16) -> Result<TestServer, Error> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
        let port = listener.local_addr()?.port();
        let resources: ServerResources = Arc::new(Mutex::new(vec!()));
        let requests_tx = Arc::new(Mutex::new(None));

        let res = Arc::clone(&resources);
        let tx = Arc::clone(&requests_tx);

        thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = stream.unwrap();

                let mut buffer = [0; 512];
                stream.peek(&mut buffer).unwrap();

                if buffer.starts_with(b"CLOSE") {
                    break;
                }

                handle_connection(&stream, res.clone(), tx.clone());
            }
        });

        Ok(TestServer{ port, resources, requests_tx })
    }

    /// Returns associated port number.
    /// ```
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    /// let server = TestServer::new().unwrap();
    ///
    /// assert!(server.port() > 0);
    /// ```
    pub fn port(&self) -> u16 {
       self.port
    }

    /// Closes listener. Server stops receiving connections. Do nothing if listener is already closed.
    ///
    /// In most the cases this method is not required as the listener is automatically closed when
    /// the value is dropped.
    ///
    /// ```
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    /// let server = TestServer::new().unwrap();
    ///
    /// server.close();
    /// ```
    pub fn close(&self) {
        if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", self.port)) {
            stream.write_all(b"CLOSE").unwrap();
            stream.flush().unwrap();
        }
    }

    /// Creates a new resource. By default resources answer "200 Ok".
    ///
    /// Check [`Resource`] for all possible configurations.
    ///
    /// ```
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    /// let server = TestServer::new().unwrap();
    /// let resource = server.create_resource("/user/settings");
    /// ```
    /// [`Resource`]: struct.Resource.html
    pub fn create_resource(&self, uri: &str) -> Resource {
        let mut resources = self.resources.lock().unwrap();
        let resource = Resource::new(uri);

        resources.push(resource.clone());

        resource
    }
    /// Creates a new resource but treats the uri as valid regex. By default resources answer "200 Ok".
    /// Suitable for use with [`Resource::body_fn`] to do more complex pattern matching and capture value extraction.
    /// Panics if the regex compiler encounters an error.
    ///
    /// Check [`Resource`] for all possible configurations.
    ///
    /// ```
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    /// let server = TestServer::new().unwrap();
    /// // for requests only accepting '/user/<uuid>' e.g. /user/e08d4d62-6252-11ef-b9c4-6f49c4a9f2f6
    /// let resource = server.create_resource_with_regex(r"/user/[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}");
    /// ```
    /// 
    /// # Example
    /// Extract section of the url that matches a particular pattern and use it to generate body data
    ///
    /// ```
    /// # use http_test_server::TestServer;
    /// # let server = TestServer::new().unwrap();
    /// // matches all uri with /{group_id}/{artifact_id}/maven-metadata.xml
    /// // the difference with `create_resource` is group_id may contain any number of '/' e.g /com/example/module-a/maven-metadata.xml
    /// let resource = server.create_resource_with_regex("^/(?<group_id>.*)/(?<artifact_id>.*)/maven-metadata.xml$");
    /// resource.body_fn(|params| {
    ///     let group_id = params.query.get("group_id").unwrap();
    ///     let artifact_id = params.query.get("artifact_id").unwrap();
    ///     assert_eq!(group_id, "com/example");
    ///     assert_eq!(artifact_id, "module-a");
    ///     // convert to package name
    ///     let group_id = group_id.replace('/', ".");
    ///     assert_eq!(group_id.as_str(), "com.example");
    ///     
    ///     return format!("<metadata><group-id>{}</group-id><artifact-id>{}</artifact-id></metadata>", group_id, artifact_id)
    ///
    /// });
    ///
    /// ```
    /// [`Resource`]: struct.Resource.html
    pub fn create_resource_with_regex(&self, uri: &str) -> Resource {
        let mut resources = self.resources.lock().unwrap();
        let re = Regex::new(uri).unwrap();
        let params: Vec<String> = re.capture_names().skip(1).filter(|n| n.is_some()).map(|n| n.unwrap().to_string()).collect();
        let resource = Resource::new_with_regex(uri, Regex::new(uri).unwrap(), URIParameters::new(params, HashMap::new()));

        resources.push(resource.clone());

        resource
        
    }

    /// Retrieves information on new requests.
    ///
    /// ```no_run
    ///# extern crate http_test_server;
    ///# use http_test_server::{TestServer, Resource};
    ///# use std::collections::HashMap;
    /// let server = TestServer::new().unwrap();
    ///
    /// for request in server.requests().iter() {
    ///     assert_eq!(request.url, "/endpoint");
    ///     assert_eq!(request.method, "GET");
    ///     assert_eq!(request.headers.get("Content-Type").unwrap(), "text");
    /// }
    /// ```
    pub fn requests(&self) -> mpsc::Receiver<Request> {
        let (tx, rx) = mpsc::channel();

        *self.requests_tx.lock().unwrap() = Some(tx);

        rx
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.close();
    }
}

fn handle_connection(stream: &TcpStream, resources: ServerResources, requests_tx: RequestsTX) {
    let stream = stream.try_clone().unwrap();

    thread::spawn(move || {
        let mut write_stream = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);

        let (method, url) = parse_request_header(&mut reader);
        let resource = find_resource(method.clone(), url.clone(), resources);

        if let Some(delay) = resource.get_delay() {
            thread::sleep(delay);
        }

        write_stream.write_all(resource.build_response(&url).as_bytes()).unwrap();
        write_stream.flush().unwrap();

        if let Some(ref tx) = *requests_tx.lock().unwrap() {
            let mut headers = HashMap::new();

            for line in reader.lines() {
                let line = line.unwrap();

                if line == "" {
                    break
                }

                let (name, value) = parse_header(line);
                headers.insert(name, value);
            }

            tx.send(Request { url, method, headers }).unwrap();
        }

        if resource.is_stream() {
            let receiver = resource.stream_receiver();
            for line in receiver.iter() {
                write_stream.write_all(line.as_bytes()).unwrap();
                write_stream.flush().unwrap();
            }
        }

    });
}

fn parse_header(message: String) -> (String, String) {
    let parts: Vec<&str> = message.splitn(2, ':').collect();
    (String::from(parts[0]), String::from(parts[1].trim()))
}

fn parse_request_header(reader: &mut dyn BufRead) -> (String, String) {
    let mut request_header = String::from("");
    reader.read_line(&mut request_header).unwrap();

    let request_header: Vec<&str> = request_header
        .split_whitespace().collect();

    (request_header[0].to_string(), request_header[1].to_string())
}

fn find_resource(method: String, url: String, resources: ServerResources) -> Resource {
    let resources = resources.lock().unwrap();

    match resources.iter().find(|r| r.matches_uri(&url) && r.get_method().equal(&method) ) {
        Some(resource) => {
            resource.increment_request_count();
            resource.clone()
        },
        None => {
            // resource not found, check whether to show 404 or MethodNotAllowed.
            let resources_for_uri = resources.iter().filter(|r| r.matches_uri(&url));
            if resources_for_uri.count() == 0 {
                return Resource::new(&url).status(Status::NotFound).clone();
            }
            Resource::new(&url).status(Status::MethodNotAllowed).clone()
        }
    }
}


/// Request information
///
/// this contains basic information about a request received.
#[derive(Debug, PartialEq)]
pub struct Request {
    /// Request URL
    pub url: String,
    /// HTTP method
    pub method: String,
    /// Request headers
    pub headers: HashMap<String, String>
}

#[cfg(test)]
mod tests {
    use std::io::prelude::*;
    use std::io::BufReader;
    use std::io::ErrorKind;
    use std::net::TcpStream;
    use std::time::Duration;
    use std::sync::mpsc;
    use super::*;

    fn make_request(port: u16, uri: &str) -> TcpStream {
       request(port, uri, "GET")
    }

    fn make_post_request(port: u16, uri: &str) -> TcpStream {
       request(port, uri, "POST")
    }

    fn request(port: u16, uri: &str, method: &str) -> TcpStream {
        let host = format!("127.0.0.1:{}", port);
        let mut stream = TcpStream::connect(host).unwrap();
        let request = format!(
            "{} {} HTTP/1.1\r\nContent-Type: text\r\n\r\n",
            method,
            uri
        );

        stream.write(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        stream
    }

    #[test]
    fn returns_404_when_requested_enexistent_resource() {
        let server = TestServer::new().unwrap();
        let stream = make_request(server.port(), "/something");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 404 Not Found\r\n");
    }

    #[test]
    fn server_should_use_random_port() {
        let server = TestServer::new().unwrap();
        let server_2 = TestServer::new().unwrap();

        assert_ne!(server.port(), server_2.port());
    }

    #[test]
    fn should_close_connection() {
        let server = TestServer::new().unwrap();
        server.close();

        thread::sleep(Duration::from_millis(200));

        let host = format!("127.0.0.1:{}", server.port());
        let stream = TcpStream::connect(host);

        assert!(stream.is_err());
        if let Err(e) = stream {
            assert_eq!(e.kind(), ErrorKind::ConnectionRefused);
        }
    }

    #[test]
    fn should_handle_multiple_resources() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/this");
        resource.status(Status::OK).body("<this body>");
        thread::sleep(Duration::from_millis(200));
        let resource2 = server.create_resource("/that");
        resource2.status(Status::OK).body("<that body>");

        assert_eq!(resource.request_count(), 0);
        assert_eq!(resource2.request_count(), 0);

        let _ = make_request(server.port(), "/this");

        thread::sleep(Duration::from_millis(200));
        let _ = make_request(server.port(), "/that");
        thread::sleep(Duration::from_millis(200));

        assert_eq!(resource.request_count(), 1);
        assert_eq!(resource2.request_count(), 1);
    }

    #[test]
    fn should_create_resource() {
        let server = TestServer::new().unwrap();
        server.create_resource("/something");

        let stream = make_request(server.port(), "/something");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n");
    }

    #[test]
    fn should_return_configured_status_for_resource_resource() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");

        resource.status(Status::OK);

        let stream = make_request(server.port(), "/something-else");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n");
    }

    #[test]
    fn should_return_resource_body() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");

        resource.status(Status::OK).body("<some body>");

        let stream = make_request(server.port(), "/something-else");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\n<some body>");
    }

    #[test]
    fn should_return_resource_body_with_params() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/user/{userId}/{thing_id}/{yepyep}");

        resource.status(Status::OK).body("User: {path.userId} Thing: {path.thing_id} Sth: {path.yepyep}");

        let stream = make_request(server.port(), "/user/123/abc/Hello!");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\nUser: 123 Thing: abc Sth: Hello!");
    }
    #[test]
    fn should_return_resource_body_with_regex_params() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource_with_regex("^/(?<group_id>.*)/(?<artifact_id>.*)/maven-metadata.xml$");

        resource.status(Status::OK).body("{path.group_id}:{path.artifact_id}");
        
        let stream = make_request(server.port(), "/com/example/http/module-a/maven-metadata.xml");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\ncom/example/http:module-a");
    }

    #[test]
    fn should_work_with_regex_uri() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/hello/[0-9]/[A-z]/.*");

        resource.method(Method::POST).status(Status::OK).body("<some body>");

        let stream = make_post_request(server.port(), "/hello/8/b/doesntmatter-hehe");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\n<some body>");
    }
    #[test]
    fn should_work_with_full_regex_uri() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource_with_regex("^/(?<group_id>.*)/(?<artifact_id>.*)/maven-metadata.xml$");

        resource.method(Method::GET).status(Status::OK)
            .body_fn(|param| {
                let group_id = param.path.get("group_id").unwrap();
                let artifact_id = param.path.get("artifact_id").unwrap();

                format!("{group_id}:{artifact_id}")
            });
        

        let stream = make_request(server.port(), "/com/example/http/module-a/maven-metadata.xml");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\ncom/example/http:module-a");
        
    }


    #[test]
    fn should_listen_to_defined_method() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");

        resource.method(Method::POST).status(Status::OK).body("<some body>");

        let stream = make_post_request(server.port(), "/something-else");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\n<some body>");
    }

    #[test]
    fn should_allow_multiple_methods_for_same_uri() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");
        let resource2 = server.create_resource("/something-else");

        resource.method(Method::GET).status(Status::OK).body("<some body GET>");
        resource2.method(Method::POST).status(Status::OK).body("<some body POST>");

        let stream_get = make_request(server.port(), "/something-else");
        let stream_post = make_post_request(server.port(), "/something-else");

        let mut reader = BufReader::new(stream_get);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        let mut reader = BufReader::new(stream_post);
        let mut line2 = String::new();
        reader.read_to_string(&mut line2).unwrap();

        assert_eq!(line, "HTTP/1.1 200 Ok\r\n\r\n<some body GET>");
        assert_eq!(line2, "HTTP/1.1 200 Ok\r\n\r\n<some body POST>");
    }

    #[test]
    fn should_return_405_when_method_not_defined() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");

        resource.method(Method::POST).status(Status::OK).body("<some body>");

        let stream = make_request(server.port(), "/something-else");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_to_string(&mut line).unwrap();

        assert_eq!(line, "HTTP/1.1 405 Method Not Allowed\r\n\r\n");
    }

    #[test]
    fn should_increment_request_count() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");

        resource.status(Status::OK).body("<some body>");

        assert_eq!(resource.request_count(), 0);

        let _ = make_request(server.port(), "/something-else");
        let _ = make_request(server.port(), "/something-else");

        thread::sleep(Duration::from_millis(200));

        assert_eq!(resource.request_count(), 2);

    }

    #[test]
    fn should_expose_stream() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");
        resource.stream();

        let (tx, rx) = mpsc::channel();

        let port = server.port();

        thread::spawn(move || {
            let stream = make_request(port, "/something-else");
            let reader = BufReader::new(stream);

            for line in reader.lines() {
                let line = line.unwrap();
                tx.send(line).unwrap();
            }
        });

        thread::sleep(Duration::from_millis(200));

        resource.send_line("hello!");
        resource.send("it's me");
        resource.send("\n");

        rx.recv().unwrap();
        rx.recv().unwrap();
        assert_eq!(rx.recv().unwrap(), "hello!");
        assert_eq!(rx.recv().unwrap(), "it's me");
    }

    #[test]
    fn should_close_client_connections() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");
        let (tx, rx) = mpsc::channel();
        let port = server.port();

        resource.stream();

        thread::spawn(move || {
            let stream = make_request(port, "/something-else");
            let reader = BufReader::new(stream);

            for _line in reader.lines() {}

            tx.send("connection closed").unwrap();
            thread::sleep(Duration::from_millis(200));
        });

        thread::sleep(Duration::from_millis(100));
        resource.close_open_connections();

        assert_eq!(rx.recv().unwrap(), "connection closed");
    }

    #[test]
    fn should_return_requests_metadata() {
        let server = TestServer::new().unwrap();
        let (tx, rx) = mpsc::channel();
        let port = server.port();

        thread::spawn(move || {
            for req in server.requests().iter() {
                tx.send(req).unwrap();
                thread::sleep(Duration::from_millis(400));
                break;
            }
        });

        thread::sleep(Duration::from_millis(100));
        let _req = make_request(port, "/something-else");

        let mut request_headers = HashMap::new();
        request_headers.insert(String::from("Content-Type"), String::from("text"));

        let expected_request = Request {
            url: String::from("/something-else"),
            method: String::from("GET"),
            headers: request_headers
        };

        assert_eq!(rx.recv().unwrap(), expected_request);
    }

    #[test]
    fn should_delay_response() {
        let server = TestServer::new().unwrap();
        let resource = server.create_resource("/something-else");
        resource.delay(Duration::from_millis(300));

        let (tx, rx) = mpsc::channel();

        let port = server.port();

        thread::spawn(move || {
            let stream = make_request(port, "/something-else");
            let reader = BufReader::new(stream);

            for line in reader.lines() {
                let line = line.unwrap();
                tx.send(line).unwrap();
            }
        });

        thread::sleep(Duration::from_millis(200));

        assert!(rx.try_recv().is_err());
        thread::sleep(Duration::from_millis(200));
        assert_eq!(rx.try_recv().unwrap(), "HTTP/1.1 200 Ok");
    }

    #[test]
    fn server_should_close_connection_when_dropped() {
        let port;
        {
            let server = TestServer::new().unwrap();
            port = server.port();
            thread::sleep(Duration::from_millis(200));
        }

        let host = format!("localhost:{}", port);
        let stream = TcpStream::connect(host);

        if let Err(e) = stream {
            assert_eq!(e.kind(), ErrorKind::ConnectionRefused);
        } else {
            panic!("connection should be closed");
        }
    }
}
