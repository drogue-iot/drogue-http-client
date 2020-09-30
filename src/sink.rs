use core::fmt::Write;
use heapless::{ArrayLength, Vec};

/// A sink to send HTTP requests to
pub trait Sink {
    fn send(&mut self, data: &[u8]) -> Result<usize, ()>;
}

/// A sink implementation for a buffer.
impl<N> Sink for Vec<u8, N>
where
    N: ArrayLength<u8>,
{
    fn send(&mut self, data: &[u8]) -> Result<usize, ()> {
        self.extend_from_slice(data).map_err(|_| ())?;

        Ok(data.len())
    }
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
