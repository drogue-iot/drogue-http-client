use crate::{Request, ResponseHandler, Sink};
use core::str::from_utf8;
use drogue_network::tcp::TcpStack;
use heapless::ArrayLength;

pub trait Source {
    type Error;

    fn pipe_data<IN, R>(&mut self, request: &mut Request<IN, R>) -> Result<(), Self::Error>
    where
        IN: ArrayLength<u8>,
        R: ResponseHandler;
}

pub struct TcpSocketSinkSource<'tcp, T>
where
    T: TcpStack,
{
    stack: &'tcp mut T,
    socket: &'tcp mut T::TcpSocket,
}

impl<'tcp, T> TcpSocketSinkSource<'tcp, T>
where
    T: TcpStack,
{
    pub fn from(stack: &'tcp mut T, socket: &'tcp mut T::TcpSocket) -> Self {
        TcpSocketSinkSource { stack, socket }
    }
}

impl<'tcp, T> Source for TcpSocketSinkSource<'tcp, T>
where
    T: TcpStack,
{
    type Error = T::Error;

    fn pipe_data<IN, R>(&mut self, request: &mut Request<IN, R>) -> Result<(), Self::Error>
    where
        IN: ArrayLength<u8>,
        R: ResponseHandler,
    {
        let mut buffer = [0u8; 512];
        while !request.is_complete() {
            match self.stack.read(self.socket, &mut buffer) {
                Ok(len) => {
                    request.push_data(&buffer[0..len]);
                }
                Err(nb::Error::WouldBlock) => {}
                Err(nb::Error::Other(e)) => return Err(e),
            }
        }
        Ok(())
    }
}

impl<'tcp, T> Sink for TcpSocketSinkSource<'tcp, T>
where
    T: TcpStack,
{
    fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
        log::info!("Sending: {:?}", from_utf8(data));
        self.stack.write(self.socket, data).map_err(|_| ())
    }
}
