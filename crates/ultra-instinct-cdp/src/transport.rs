use crate::CdpError;

/// One unsolicited CDP event frame (no `id`, has `method` + `params`).
///
/// Buffered by transports that see the wire (the websocket reader drops
/// events that arrive between commands only when nobody drains them).
#[derive(Clone, Debug)]
pub struct CdpEvent {
    pub method: String,
    pub params: serde_json::Value,
}

/// One CDP command/response exchange.
///
/// A live websocket and a replay script both implement this. `params_json`
/// is a JSON object. The returned string is the CDP `result` value, not the
/// envelope. Implementations must not invent a click the caller did not ask for.
pub trait CdpTransport {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError>;

    /// Drain unsolicited events buffered since the last drain. Default: none —
    /// replay transports produce no events.
    fn drain_events(&mut self) -> Vec<CdpEvent> {
        Vec::new()
    }
}

/// A boxed transport is a transport, so a server can hold a live websocket or
/// a replay behind one type.
impl<T: CdpTransport + ?Sized> CdpTransport for Box<T> {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        (**self).call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        (**self).drain_events()
    }
}
