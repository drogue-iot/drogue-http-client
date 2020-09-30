use crate::{Request, ResponseHandler};
use heapless::ArrayLength;

/// A source of data for the HTTP response
pub trait Source {
    type Error;

    /// This will block, and forward data from this source to the request, until the request
    /// is completed or a read error occurred.
    fn pipe_data<IN, R>(&mut self, request: &mut Request<IN, R>) -> Result<(), Self::Error>
    where
        IN: ArrayLength<u8>,
        R: ResponseHandler;
}
