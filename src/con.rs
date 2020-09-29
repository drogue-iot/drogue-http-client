use crate::{NoOpResponseHandler, Response, ResponseHandler, Sink};
use core::str::from_utf8;
use heapless::{ArrayLength, Vec};
use httparse::Status;

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

#[derive(Copy, Clone, Debug)]
enum State {
    Header,
    Payload(usize),
    Complete,
    UnlimitedPayload,
}

pub struct Request<IN, R>
where
    IN: ArrayLength<u8>,
    R: ResponseHandler,
{
    // connection
    pub(crate) connection: HttpConnection<IN>,
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
