//! The little HTTP the Drive provider needs, as a trait: the real transport is WinHTTP (winhttp.rs), the tests
//! script requests and answers (`FakeHttp` below) so every Drive rule is checked without a network.
use std::io::{self, Read, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

pub enum Body<'a> {
    Empty,
    Bytes(Vec<u8>),
    /// Exactly `len` bytes read from `reader` (a chunk of a video).
    Stream { reader: &'a mut dyn Read, len: u64 },
}

pub struct Request<'a> {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Body<'a>,
    /// Off for resumable uploads: Google answers "308 Resume Incomplete", which is not a redirect.
    pub follow_redirects: bool,
}

impl<'a> Request<'a> {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self { method, url: url.into(), headers: Vec::new(), body: Body::Empty, follow_redirects: true }
    }
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }
    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", format!("Bearer {token}"))
    }
    pub fn json(self, value: &serde_json::Value) -> Self {
        let mut r = self.header("Content-Type", "application/json; charset=UTF-8");
        r.body = Body::Bytes(value.to_string().into_bytes());
        r
    }
    pub fn form(self, pairs: &[(&str, &str)]) -> Self {
        let body = form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        let mut r = self.header("Content-Type", "application/x-www-form-urlencoded");
        r.body = Body::Bytes(body.into_bytes());
        r
    }
    pub fn bytes(mut self, content_type: &str, bytes: Vec<u8>) -> Self {
        self = self.header("Content-Type", content_type);
        self.body = Body::Bytes(bytes);
        self
    }
    pub fn stream(mut self, reader: &'a mut dyn Read, len: u64) -> Self {
        self.body = Body::Stream { reader, len };
        self
    }
    pub fn no_redirects(mut self) -> Self {
        self.follow_redirects = false;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    /// The body of a non-2xx answer (Google's error JSON). A 2xx body goes to the sink instead, so an error
    /// page is never written into a downloaded video.
    pub error_body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait HttpClient: Send + Sync {
    /// Sends the request; a 2xx body is streamed into `sink`, any other body lands in `error_body`.
    fn send(&self, request: Request, sink: &mut dyn Write) -> io::Result<Response>;
}

/// Sends and collects the body (small JSON answers).
pub fn send_collect(http: &dyn HttpClient, request: Request) -> io::Result<(Response, Vec<u8>)> {
    let mut body = Vec::new();
    let response = http.send(request, &mut body)?;
    if !response.ok() {
        body = response.error_body.clone();
    }
    Ok((response, body))
}

/// `?a=b&c=d` with every value percent-encoded.
pub fn query(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        out.push(if i == 0 { '?' } else { '&' });
        out.push_str(k);
        out.push('=');
        out.extend(percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC));
    }
    out
}

/// Scripted HTTP for tests: each request is checked by its handler in order, which answers it.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// What a test sees of a request (the body read out).
    #[derive(Debug, Clone)]
    pub struct Seen {
        pub method: Method,
        pub url: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
        pub follow_redirects: bool,
    }

    impl Seen {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }
        pub fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }
    }

    pub type Handler = Box<dyn FnOnce(&Seen) -> (u16, Vec<(&'static str, String)>, Vec<u8>) + Send>;

    #[derive(Default)]
    pub struct FakeHttp {
        script: Mutex<VecDeque<Handler>>,
        pub log: Mutex<Vec<Seen>>,
    }

    impl FakeHttp {
        pub fn expect(&self, handler: impl FnOnce(&Seen) -> (u16, Vec<(&'static str, String)>, Vec<u8>) + Send + 'static) {
            self.script.lock().unwrap().push_back(Box::new(handler));
        }
        pub fn remaining(&self) -> usize {
            self.script.lock().unwrap().len()
        }
    }

    impl HttpClient for FakeHttp {
        fn send(&self, request: Request, sink: &mut dyn Write) -> io::Result<Response> {
            let body = match request.body {
                Body::Empty => Vec::new(),
                Body::Bytes(b) => b,
                Body::Stream { reader, len } => {
                    let mut b = vec![0; len as usize];
                    reader.read_exact(&mut b)?;
                    b
                }
            };
            let seen = Seen {
                method: request.method,
                url: request.url,
                headers: request.headers,
                body,
                follow_redirects: request.follow_redirects,
            };
            let handler = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected request {} {}", seen.method.as_str(), seen.url));
            let (status, headers, body) = handler(&seen);
            self.log.lock().unwrap().push(seen);
            let headers = headers.into_iter().map(|(n, v)| (n.to_ascii_lowercase(), v)).collect();
            if (200..300).contains(&status) {
                sink.write_all(&body)?;
                Ok(Response { status, headers, error_body: Vec::new() })
            } else {
                Ok(Response { status, headers, error_body: body })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_strings_are_encoded() {
        assert_eq!(query(&[("q", "name='Lag' and trashed=false"), ("x", "1")]), "?q=name%3D%27Lag%27%20and%20trashed%3Dfalse&x=1");
    }
}
