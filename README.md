# Drogue IoT HTTP Client

[![CI](https://github.com/drogue-iot/drogue-http-client/workflows/CI/badge.svg)](https://github.com/drogue-iot/drogue-http-client/actions?query=workflow%3A%22CI%22)
[![crates.io](https://img.shields.io/crates/v/drogue-http-client.svg)](https://crates.io/crates/drogue-http-client)
[![docs.rs](https://docs.rs/drogue-http-client/badge.svg)](https://docs.rs/drogue-http-client)
[![Matrix](https://img.shields.io/matrix/drogue-iot:matrix.org)](https://matrix.to/#/#drogue-iot:matrix.org)

An HTTP client for embedded systems, based on [drogue-network](https://github.com/drogue-iot/drogue-network).
It is intended to be used in a `#![no_std]` environment, without an allocator. 

## To Do

**NOTE:** While is says "HTTP", it means "something that, with a bit of luck, could be interpreted as HTTP".
It is far from a full HTTP 1.1 client.

* [ ] Handle errors
* [ ] Implement chunked encoding
* [ ] Lots more …

## Example

~~~rust
const ENDPOINT: &'static str = "my-host";

fn send() -> Result<(),()> {

  // socket from drogue-network, maybe with TLS
  let mut tcp = TcpSocketSinkSource::from(network, socket);

  let con = HttpConnection::<U1024>::new();

  let data = r#"{"temp": 1.23}"#;

  // response implementation with buffer 
  let handler = BufferResponseHandler::<U1024>::new();

  // create and execute request
  let mut req = con
    .post("/publish/telemetry")
    .headers(&[("Host", ENDPOINT), ("Content-Type", "text/json")])
    .handler(handler)
    .execute_with::<_, consts::U512>(&mut tcp, Some(data.as_bytes()));

  tcp.pipe_data(&mut req)
    .map_err(|_| ThingError::FailedToPublish)?;
    
  let (con, handler) = req.complete();
    
  log::info!(
    "Result: {} {}, Payload: {:?}",
    handler.code(),
    handler.reason(),
    from_utf8(handler.payload())
  );

  // you can do the next call with the returned `con`
}
~~~