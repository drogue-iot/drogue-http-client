#![no_std]

mod con;
mod response;
mod sink;
pub mod tcp;

pub use con::*;
pub use response::*;
pub use sink::*;

#[cfg(test)]
mod test {
    use super::*;
    use core::str::from_utf8;
    use heapless::consts::*;
    use heapless::{ArrayLength, String, Vec};

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
