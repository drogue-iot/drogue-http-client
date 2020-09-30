//! Implementations just for the sake of creating compilable documentation.

use drogue_network::addr::HostSocketAddr;
use drogue_network::tcp::{Mode, TcpError, TcpStack};

pub struct MockStack {}

pub struct MockSocket {}

#[derive(Debug)]
pub enum MockError {}

impl From<MockError> for TcpError {
    fn from(_: MockError) -> Self {
        unimplemented!()
    }
}

impl From<MockError> for () {
    fn from(_: MockError) -> Self {
        unimplemented!()
    }
}

pub fn mock_connection() -> (MockStack, MockSocket) {
    (MockStack {}, MockSocket {})
}

impl TcpStack for MockStack {
    type TcpSocket = MockSocket;
    type Error = MockError;

    fn open(&self, _: Mode) -> Result<Self::TcpSocket, Self::Error> {
        unimplemented!()
    }

    fn connect(
        &self,
        _: Self::TcpSocket,
        _: HostSocketAddr,
    ) -> Result<Self::TcpSocket, Self::Error> {
        unimplemented!()
    }

    fn is_connected(&self, _: &Self::TcpSocket) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn write(&self, _: &mut Self::TcpSocket, _: &[u8]) -> nb::Result<usize, Self::Error> {
        unimplemented!()
    }

    fn read(&self, _: &mut Self::TcpSocket, _: &mut [u8]) -> nb::Result<usize, Self::Error> {
        unimplemented!()
    }

    fn close(&self, _: Self::TcpSocket) -> Result<(), Self::Error> {
        unimplemented!()
    }
}
