use crate::Response;

use heapless::consts;
use heapless::String;
use heapless::{ArrayLength, Vec};

/// A no-op response handler.
pub struct NoOpResponseHandler;

impl ResponseHandler for NoOpResponseHandler {
    fn response(&mut self, _: Response) {}
    fn more_payload(&mut self, _: Result<Option<&[u8]>, ()>) {}
}

/// A trait handling responses to an HTTP request.
pub trait ResponseHandler {
    fn response(&mut self, response: Response);
    fn more_payload(&mut self, payload: Result<Option<&[u8]>, ()>);
}

/// A response handler, that will buffer all data.
pub struct BufferResponseHandler<N, NR = consts::U128>
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
