#![no_std]

pub mod tcp;

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

impl<N> Sink for Vec<u8, N>
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

pub struct HttpConnection<IN>
where
    IN: ArrayLength<u8>,
{
    // inbound transport buffer
    inbound: Vec<u8, IN>,
}

impl<IN> HttpConnection<IN>
where
    IN: ArrayLength<u8>,
{
    pub fn new() -> Self {
        HttpConnection {
            inbound: Vec::new(),
        }
    }

    pub fn begin<'req>(
        self,
        method: &'static str,
        path: &'static str,
    ) -> RequestBuilder<'req, IN, NoOpResponseHandler> {
        log::debug!("Begin new request - method: {}, path: {}", method, path);

        RequestBuilder {
            connection: self,
            method,
            path,
            headers: None,
            handler: NoOpResponseHandler,
        }
    }

    pub fn post<'req>(self, path: &'static str) -> RequestBuilder<'req, IN, NoOpResponseHandler> {
        self.begin("POST", path)
    }

    pub(crate) fn send_request<S, OUT>(
        &mut self,
        sink: &mut S,
        method: &str,
        path: &str,
        headers: Option<&[(&str, &str)]>,
        payload: Option<&[u8]>,
    ) -> Result<(), ()>
    where
        S: Sink,
        OUT: ArrayLength<u8>,
    {
        let mut out = Vec::<u8, OUT>::new();

        // create headers
        self.create_request_headers(&mut out, method, path, headers, payload.map(|b| b.len()))
            .map_err(|_| ())?;

        // send headers
        sink.send(&out)?;

        // send payload
        if let Some(payload) = payload {
            sink.send(payload)?;
        }

        Ok(())
    }

    fn create_request_headers(
        &self,
        w: &mut dyn core::fmt::Write,
        method: &str,
        path: &str,
        headers: Option<&[(&str, &str)]>,
        content_length: Option<usize>,
    ) -> Result<(), core::fmt::Error> {
        write!(w, "{} {} HTTP/1.1\r\n", method, path)?;
        if let Some(headers) = headers {
            if let Some(content_length) = content_length {
                write!(w, "{}: {}\r\n", "Content-Length", content_length)?;
            }
            for header in headers {
                write!(w, "{}: {}\r\n", header.0, header.1)?;
            }
        }
        write!(w, "\r\n")?;

        Ok(())
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

pub struct RequestBuilder<'req, IN, R>
where
    IN: ArrayLength<u8>,
    R: ResponseHandler,
{
    connection: HttpConnection<IN>,
    method: &'static str,
    path: &'static str,
    headers: Option<&'req [(&'req str, &'req str)]>,
    handler: R,
}

impl<'req, IN, R> RequestBuilder<'req, IN, R>
where
    IN: ArrayLength<u8>,
    R: ResponseHandler,
{
    pub fn headers(mut self, headers: &'req [(&'req str, &'req str)]) -> Self {
        self.headers = Some(headers);
        self
    }

    pub fn handler<RN: ResponseHandler>(self, handler: RN) -> RequestBuilder<'req, IN, RN> {
        RequestBuilder {
            connection: self.connection,
            headers: self.headers,
            method: self.method,
            path: self.path,
            handler,
        }
    }

    pub fn execute<S, OUT>(self, sink: &mut S) -> Request<IN, R>
    where
        S: Sink,
        OUT: ArrayLength<u8>,
    {
        self.execute_with::<S, OUT>(sink, None)
    }

    pub fn execute_with<S, OUT>(mut self, sink: &mut S, payload: Option<&[u8]>) -> Request<IN, R>
    where
        S: Sink,
        OUT: ArrayLength<u8>,
    {
        // FIXME: handle error
        self.connection
            .send_request::<S, OUT>(sink, self.method, self.path, self.headers, payload);
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

pub struct Request<IN, R>
where
    IN: ArrayLength<u8>,
    R: ResponseHandler,
{
    // connection
    connection: HttpConnection<IN>,
    // current handler
    handler: R,
    // current state
    state: State,
    // processed bytes
    processed_bytes: usize,
}

impl<IN, R> Request<IN, R>
where
    IN: ArrayLength<u8>,
    R: ResponseHandler,
{
    /// Check if the request is processed completely
    pub fn is_complete(&self) -> bool {
        match self.state {
            State::Complete => true,
            _ => false,
        }
    }

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
        log::debug!("Current data: {:?}", from_utf8(&self.connection.inbound));

        match data {
            Ok(Some(data)) => {
                self.connection.inbound.extend_from_slice(data).ok();

                let mut headers = [httparse::EMPTY_HEADER; 16];
                let mut response = httparse::Response::new(&mut headers);

                match response.parse(&self.connection.inbound) {
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

                        let buffer_len = self.connection.inbound.len();
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

                        self.connection.inbound.clear();
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
                self.connection.inbound.extend_from_slice(data);
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

    pub fn complete(self) -> (HttpConnection<IN>, R) {
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
        let con = HttpConnection::<U1024>::new();

        let headers = [("Content-Type", "text/json")];

        let handler = BufferResponseHandler::<U1024>::new();

        let mut req = {
            con.post("/foo.bar")
                .headers(&headers)
                .handler(handler)
                .execute::<_, U128>(&mut sink_buffer)
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
            None,
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
            None,
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
            None,
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
            None,
            b"POST / HTTP/1.1\r\nContent-Type: text/json\r\n\r\n",
            &[b"HTTP/1.1 200 OK\r\n\r\n0123456789"],
            200,
            "OK",
            b"0123456789",
        );
    }

    #[test]
    fn simple_send_payload() {
        assert_http(
            "POST",
            "/",
            &[("Content-Type", "text/json")],
            Some(b"0123456789"),
            b"POST / HTTP/1.1\r\nContent-Length: 10\r\nContent-Type: text/json\r\n\r\n0123456789",
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
        let mut mock_sink = MockSinkImpl::<U1024>::new(expected);

        let con = HttpConnection::<U1024>::new();

        let con = assert_request(
            con,
            &mut mock_sink,
            "POST",
            "/",
            &[("Content-Type", "text/plain")],
            None,
            &[b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789"],
            false,
            200,
            "OK",
            b"0123456789",
        );

        assert_request(
            con,
            &mut mock_sink,
            "POST",
            "/",
            &[("Content-Type", "text/plain")],
            None,
            &[b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789"],
            true,
            200,
            "OK",
            b"0123456789",
        );
    }

    fn assert_request<IN, S>(
        con: HttpConnection<IN>,
        sink: &mut S,
        method: &'static str,
        path: &'static str,
        headers: &[(&str, &str)],
        payload: Option<&[u8]>,
        push: &[&[u8]],
        close_after_push: bool,
        code: u16,
        reason: &str,
        expected_payload: &[u8],
    ) -> HttpConnection<IN>
    where
        IN: ArrayLength<u8>,
        S: Sink + MockSink,
    {
        // capture response output

        let handler = BufferResponseHandler::<U1024>::new();

        // begin request

        let mut req = {
            con.begin(method, path)
                .headers(&headers)
                .handler(handler)
                .execute_with::<_, U1024>(sink, payload)
        };

        // mock response

        for p in push {
            req.push_data(p);
        }

        if close_after_push {
            req.push_close();
        }

        // close request

        let (con, handler) = req.complete();

        // assert sink

        sink.assert();

        // assert response

        assert_eq!(code, handler.code());
        assert_eq!(reason, handler.reason());

        assert_eq!(
            core::str::from_utf8(handler.payload()),
            core::str::from_utf8(expected_payload)
        );

        assert!(handler.is_complete());

        con
    }

    fn assert_http<'m>(
        method: &'static str,
        path: &'static str,
        headers: &[(&str, &str)],
        payload: Option<&[u8]>,
        expected_sink: &'m [u8],
        push: &[&[u8]],
        code: u16,
        reason: &str,
        expected_payload: &[u8],
    ) {
        // capture sink output

        let expected = &[expected_sink];
        let mut mock_sink = MockSinkImpl::<U1024>::new(expected);

        let con = HttpConnection::<U1024>::new();

        assert_request(
            con,
            &mut mock_sink,
            method,
            path,
            headers,
            payload,
            push,
            true,
            code,
            reason,
            expected_payload,
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
