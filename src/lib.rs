#![no_std]

use core::fmt::Write;
use heapless::{ArrayLength, String, Vec};
use httparse::Status;

pub trait Sink {
    fn send(&mut self, data: &[u8]) -> Result<usize, ()>;
}

struct SinkWrapper<'a>(&'a mut dyn Sink);

impl<'a> Write for SinkWrapper<'a> {
    fn write_str(&mut self, s: &str) -> Result<(), core::fmt::Error> {
        let buffer = s.as_bytes();
        let mut pos = 0usize;

        while pos < buffer.len() {
            match self.0.send(&buffer[pos..]) {
                Ok(len) => pos += len,
                Err(_) => return Err(core::fmt::Error),
            }
        }

        Ok(())
    }
}

impl<N> Sink for &mut Vec<u8, N>
where
    N: ArrayLength<u8>,
{
    fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
        self.extend_from_slice(data).map_err(|_| ())?;

        Ok(data.len())
    }
}

#[derive(Copy, Clone, Debug)]
enum State {
    Header,
    Payload(usize),
    Complete,
    UnlimitedPayload,
}

pub struct HttpConnection<N, S>
where
    N: ArrayLength<u8>,
    S: Sink,
{
    // sink
    sink: S,
    // inbound transport buffer
    buffer: Vec<u8, N>,
}

impl<N, S> HttpConnection<N, S>
where
    N: ArrayLength<u8>,
    S: Sink,
{
    pub fn new(sink: S) -> Self {
        HttpConnection {
            sink,
            buffer: Vec::new(),
        }
    }

    pub fn begin<'req>(
        self,
        method: &'static str,
        path: &'static str,
    ) -> RequestBuilder<'req, N, S, NoOpResponseHandler> {
        log::debug!("Begin new request - method: {}, path: {}", method, path);

        RequestBuilder {
            connection: self,
            method,
            path,
            headers: None,
            handler: NoOpResponseHandler,
        }
    }

    pub fn post<'req>(self, path: &'static str) -> RequestBuilder<'req, N, S, NoOpResponseHandler> {
        self.begin("POST", path)
    }

    pub(crate) fn send_request(
        &mut self,
        method: &str,
        path: &str,
        headers: Option<&[(&str, &str)]>,
    ) {
        // FIXME: handle write errors
        let mut sw = SinkWrapper(&mut self.sink);
        write!(sw, "{} {} HTTP/1.1\r\n", method, path);
        if let Some(headers) = headers {
            for header in headers {
                write!(sw, "{}: {}\r\n", header.0, header.1);
            }
        }
        write!(sw, "\r\n");
    }

    #[allow(dead_code)]
    pub(crate) fn with_sink<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut S),
    {
        f(&mut self.sink)
    }

    pub(crate) fn closed(&mut self) {
        // FIXME: mark as closed
    }
}

pub struct NoOpResponseHandler;

impl ResponseHandler for NoOpResponseHandler {
    fn response(&mut self, _: Response) {}
    fn more_payload(&mut self, _: Result<Option<&[u8]>, ()>) {}
}

#[derive(Debug)]
pub struct Response<'a> {
    pub version: u8,
    pub code: u16,
    pub reason: &'a str,
}

pub trait ResponseHandler {
    fn response(&mut self, response: Response);
    fn more_payload(&mut self, payload: Result<Option<&[u8]>, ()>);
}

pub struct RequestBuilder<'req, N, S, R>
where
    N: ArrayLength<u8>,
    S: Sink,
    R: ResponseHandler,
{
    connection: HttpConnection<N, S>,
    method: &'static str,
    path: &'static str,
    headers: Option<&'req [(&'req str, &'req str)]>,
    handler: R,
}

impl<'req, N, S, R> RequestBuilder<'req, N, S, R>
where
    N: ArrayLength<u8>,
    S: Sink,
    R: ResponseHandler,
{
    pub fn headers(mut self, headers: &'req [(&'req str, &'req str)]) -> Self {
        self.headers = Some(headers);
        self
    }

    pub fn handler<RN: ResponseHandler>(self, handler: RN) -> RequestBuilder<'req, N, S, RN> {
        RequestBuilder {
            connection: self.connection,
            headers: self.headers,
            method: self.method,
            path: self.path,
            handler,
        }
    }

    pub fn execute(mut self) -> Request<N, S, R> {
        self.connection
            .send_request(self.method, self.path, self.headers);
        let connection = self.connection;
        let handler = self.handler;
        Request {
            connection,
            handler,
            state: State::Header,
            processed_bytes: 0,
        }
    }
}

pub struct Request<N, S, R>
where
    N: ArrayLength<u8>,
    S: Sink,
    R: ResponseHandler,
{
    // connection
    connection: HttpConnection<N, S>,
    // current handler
    handler: R,
    // current state
    state: State,
    // processed bytes
    processed_bytes: usize,
}

impl<N, S, R> Request<N, S, R>
where
    N: ArrayLength<u8>,
    S: Sink,
    R: ResponseHandler,
{
    fn push(&mut self, data: Result<Option<&[u8]>, ()>) {
        log::debug!("Pushing data: {:?}", data.map(|o| o.map(|b| from_utf8(b))),);
        match self.state {
            State::Header => self.push_header(data),
            State::Payload(size) => self.push_sized_payload(size, data),
            State::UnlimitedPayload => self.push_payload(data),
            State::Complete => self.push_complete_payload(data),
        }
    }

    fn push_header(&mut self, data: Result<Option<&[u8]>, ()>) {
        log::debug!("Current data: {:?}", from_utf8(&self.connection.buffer));

        match data {
            Ok(Some(data)) => {
                self.connection.buffer.extend_from_slice(data).ok();

                let mut headers = [httparse::EMPTY_HEADER; 16];
                let mut response = httparse::Response::new(&mut headers);

                match response.parse(&self.connection.buffer) {
                    Ok(Status::Complete(len)) => {
                        log::debug!("Completed({})", len);

                        let content_size = response
                            .headers
                            .iter()
                            .find(|e| e.name.eq_ignore_ascii_case("content-length"));

                        // eval next state
                        // FIXME: handle error
                        self.state = match content_size {
                            Some(header) => from_utf8(header.value)
                                .map_err(|_| ())
                                .and_then(|v| v.parse::<usize>().map_err(|_| ()))
                                .map_or(State::UnlimitedPayload, |size| State::Payload(size)),
                            None => State::UnlimitedPayload,
                        };

                        // log::debug!("Headers: {:?}", response.headers);
                        log::debug!("Continue with: {:?}", self.state);

                        // handle response
                        self.handler.response(Response {
                            version: response.version.unwrap_or_default(),
                            code: response.code.unwrap_or_default(),
                            reason: response.reason.unwrap_or_default(),
                        });

                        // clear connection buffer

                        let buffer_len = self.connection.buffer.len();
                        let data_len = data.len();

                        log::debug!("Len = {}, dLen = {}, bLen = {}", len, data_len, buffer_len);

                        // push on remaining data

                        let start = len - (buffer_len - data_len);
                        let rem_data = &data[start..];

                        log::debug!(
                            "Push bytes [{}..] after header to payload processing",
                            start
                        );

                        self.push(Ok(Some(rem_data)));

                        // clear buffer

                        self.connection.buffer.clear();
                    }
                    Ok(Status::Partial) => {}
                    Err(e) => {
                        log::info!("Parse error: {:?}", e);
                    }
                }
            }
            Ok(None) => {
                // FIXME: handle close
            }
            Err(_) => {
                // FIXME: handle error
            }
        }
    }

    fn push_payload(&mut self, data: Result<Option<&[u8]>, ()>) {
        log::debug!("More data: {:?}", data);

        self.handler.more_payload(data);
    }

    fn push_complete_payload(&mut self, data: Result<Option<&[u8]>, ()>) {
        log::debug!("More data (overflow): {:?}", data);
        match data {
            Ok(Some(data)) => {
                // FIXME: handle error
                self.connection.buffer.extend_from_slice(data);
            }
            Ok(None) | Err(_) => self.connection.closed(),
        }
    }

    fn push_sized_payload(&mut self, expected_bytes: usize, data: Result<Option<&[u8]>, ()>) {
        log::debug!("More data (sized): {:?}", data);

        match data {
            Ok(Some(data)) => {
                let len = data.len();
                let rem = expected_bytes - self.processed_bytes;
                if len >= rem {
                    self.handler.more_payload(Ok(Some(&data[0..rem])));
                    // mark as complete
                    self.state = State::Complete;
                    // notify about complete
                    self.handler.more_payload(Ok(None));
                } else {
                    self.handler.more_payload(Ok(Some(data)));
                    self.processed_bytes += len;
                }
            }
            Ok(None) => {
                // FIXME: check for error
            }
            Err(_) => {}
        }
    }

    pub fn push_data(&mut self, data: &[u8]) {
        self.push(Ok(Some(data)))
    }

    pub fn push_close(&mut self) {
        self.push(Ok(None))
    }

    pub fn complete(self) -> (HttpConnection<N, S>, R) {
        (self.connection, self.handler)
    }
}

use core::str::from_utf8;
use heapless::consts::U128;

pub struct BufferResponseHandler<N, NR = U128>
where
    N: ArrayLength<u8>,
    NR: ArrayLength<u8>,
{
    version: u8,
    code: u16,
    reason: Option<String<NR>>,
    payload: Vec<u8, N>,
    complete: bool,
}

impl<N> BufferResponseHandler<N>
where
    N: ArrayLength<u8>,
{
    pub fn new() -> Self {
        BufferResponseHandler {
            version: 0u8,
            code: 0u16,
            reason: None,
            payload: Vec::new(),
            complete: false,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn code(&self) -> u16 {
        self.code
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn reason(&self) -> &str {
        self.reason.as_ref().map_or("", |s| s.as_str())
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

impl<N> ResponseHandler for BufferResponseHandler<N>
where
    N: ArrayLength<u8>,
{
    fn response(&mut self, response: Response<'_>) {
        self.version = response.version;
        self.code = response.code;
        self.reason = Some(String::from(response.reason));
    }

    fn more_payload(&mut self, payload: Result<Option<&[u8]>, ()>) {
        match payload {
            Ok(Some(data)) => {
                log::debug!("Append payload data: {:?}", data);
                self.payload.extend_from_slice(data).ok();
            }
            Ok(None) => {
                log::debug!("Complete response");
                self.complete = true;
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use core::str::from_utf8;
    use heapless::consts::*;
    use heapless::String;

    fn init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn idea() -> Result<(), ()> {
        init();

        let mut sink_buffer = Vec::<u8, U1024>::new();
        let con = HttpConnection::<U1024, _>::new(&mut sink_buffer);

        let headers = [("Content-Type", "text/json")];

        let handler = BufferResponseHandler::<U1024>::new();

        let mut req = {
            con.post("/foo.bar")
                .headers(&headers)
                .handler(handler)
                .execute()
        };

        // mock response

        req.push_data(b"HTTP/1.1 ");
        req.push_data(b"200 OK\r\n");
        req.push_data(b"\r\n");
        req.push_data(b"123");
        req.push_close();

        let (_, handler) = req.complete();

        // sink

        assert_eq!(
            String::from_utf8(sink_buffer).unwrap().as_str(),
            "POST /foo.bar HTTP/1.1\r\nContent-Type: text/json\r\n\r\n",
        );

        // result

        assert_eq!(200, handler.code());
        assert_eq!("OK", handler.reason());
        assert_eq!(core::str::from_utf8(handler.payload()), Ok("123"));

        assert!(handler.is_complete());

        // done

        Ok(())
    }

    #[test]
    fn simple() {
        assert_http(
            "POST",
            "/",
            &[],
            b"POST / HTTP/1.1\r\n\r\n",
            &[b"HTTP/1.1 200 OK\r\n\r\n0123456789"],
            200,
            "OK",
            b"0123456789",
        );
    }

    #[test]
    fn simple_split_1() {
        assert_http(
            "POST",
            "/",
            &[],
            b"POST / HTTP/1.1\r\n\r\n",
            &[b"HTTP/1.1 200 OK\r\n\r\n01234", b"56789"],
            200,
            "OK",
            b"0123456789",
        );
    }

    #[test]
    fn simple_split_2() {
        assert_http(
            "POST",
            "/",
            &[],
            b"POST / HTTP/1.1\r\n\r\n",
            &[b"HTTP/1.1 200 ", b"OK\r\n\r\n01234", b"56789"],
            200,
            "OK",
            b"0123456789",
        );
    }

    #[test]
    fn simple_header() {
        assert_http(
            "POST",
            "/",
            &[("Content-Type", "text/json")],
            b"POST / HTTP/1.1\r\nContent-Type: text/json\r\n\r\n",
            &[b"HTTP/1.1 200 OK\r\n\r\n0123456789"],
            200,
            "OK",
            b"0123456789",
        );
    }

    #[test]
    fn multiple() {
        let expected = &[
            &b"POST / HTTP/1.1\r\nContent-Type: text/plain\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nContent-Type: text/plain\r\n\r\n"[..],
        ];
        let mock_sink = MockSinkImpl::<U1024>::new(expected);

        let con = HttpConnection::<U1024, _>::new(mock_sink);

        let con = assert_request(
            con,
            "POST",
            "/",
            &[("Content-Type", "text/plain")],
            &[b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789"],
            false,
            200,
            "OK",
            b"0123456789",
        );

        assert_request(
            con,
            "POST",
            "/",
            &[("Content-Type", "text/plain")],
            &[b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789"],
            true,
            200,
            "OK",
            b"0123456789",
        );
    }

    fn assert_request<N, S>(
        con: HttpConnection<N, S>,
        method: &'static str,
        path: &'static str,
        headers: &[(&str, &str)],
        push: &[&[u8]],
        close_after_push: bool,
        code: u16,
        reason: &str,
        payload: &[u8],
    ) -> HttpConnection<N, S>
    where
        N: ArrayLength<u8>,
        S: Sink + MockSink,
    {
        // capture response output

        let handler = BufferResponseHandler::<U1024>::new();

        // begin request

        let mut req = {
            con.begin(method, path)
                .headers(&headers)
                .handler(handler)
                .execute()
        };

        // mock response

        for p in push {
            req.push_data(p);
        }

        if close_after_push {
            req.push_close();
        }

        // close request

        let (mut con, handler) = req.complete();

        // assert sink

        con.with_sink(|sink| {
            sink.assert();
        });

        // assert response

        assert_eq!(code, handler.code());
        assert_eq!(reason, handler.reason());

        assert_eq!(
            core::str::from_utf8(handler.payload()),
            core::str::from_utf8(payload)
        );

        assert!(handler.is_complete());

        con
    }

    fn assert_http<'m>(
        method: &'static str,
        path: &'static str,
        headers: &[(&str, &str)],
        expected_sink: &'m [u8],
        push: &[&[u8]],
        code: u16,
        reason: &str,
        payload: &[u8],
    ) {
        // capture sink output

        let expected = &[expected_sink];
        let mock_sink = MockSinkImpl::<U1024>::new(expected);

        let con = HttpConnection::<U1024, _>::new(mock_sink);

        assert_request(
            con, method, path, headers, push, true, code, reason, payload,
        );
    }

    pub(crate) struct MockSinkImpl<'m, N>
    where
        N: ArrayLength<u8>,
    {
        buffer: Vec<u8, N>,
        iter: core::slice::Iter<'m, &'m [u8]>,
    }

    impl<'m, N> MockSinkImpl<'m, N>
    where
        N: ArrayLength<u8>,
    {
        pub fn new(expected: &'m [&'m [u8]]) -> Self {
            let i = expected.iter();
            MockSinkImpl {
                buffer: Vec::<u8, N>::new(),
                iter: i,
            }
        }
    }

    impl<'m, N> Sink for MockSinkImpl<'m, N>
    where
        N: ArrayLength<u8>,
    {
        fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
            (&mut self.buffer).send(data)
        }
    }

    pub trait MockSink {
        fn assert(&mut self);
    }

    impl<'m, N> MockSink for MockSinkImpl<'m, N>
    where
        N: ArrayLength<u8>,
    {
        fn assert(&mut self) {
            let expected = self.iter.next();

            // assert

            assert_eq!(
                expected.and_then(|b| from_utf8(b).ok()),
                from_utf8(self.buffer.as_ref()).ok(),
            );

            // now clear the buffer
            self.buffer.clear();
        }
    }
}
