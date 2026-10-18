use std::net::TcpListener;
use std::thread;

use serde_json::{json, Value};
use tungstenite::{accept, Message};
use ultra_instinct_cdp::{CdpTransport, WebSocketTransport};

#[test]
fn call_waits_past_more_than_64_events_for_its_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = accept(stream).unwrap();
        // `connect` opens with Emulation.setFocusEmulationEnabled.
        let request = match socket.read().unwrap() {
            Message::Text(text) => serde_json::from_str::<Value>(text.as_ref()).unwrap(),
            message => panic!("expected request text, got {message:?}"),
        };
        assert_eq!(request["method"], "Emulation.setFocusEmulationEnabled");
        socket
            .send(Message::Text(
                json!({"id": request["id"].as_i64().unwrap(), "result": {}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        let request = match socket.read().unwrap() {
            Message::Text(text) => serde_json::from_str::<Value>(text.as_ref()).unwrap(),
            message => panic!("expected request text, got {message:?}"),
        };
        assert_eq!(request["method"], "Runtime.evaluate");
        let id = request["id"].as_i64().unwrap();
        for _ in 0..500 {
            socket
                .send(Message::Text(
                    r#"{"method":"CSS.styleSheetAdded","params":{}}"#.into(),
                ))
                .unwrap();
        }
        socket
            .send(Message::Text(
                json!({"id": id, "result": {"ok": true}}).to_string().into(),
            ))
            .unwrap();
    });

    let mut client = WebSocketTransport::connect(&format!("ws://{address}")).unwrap();
    let result = client.call("Runtime.evaluate", "{}").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap(),
        json!({"ok": true})
    );
    server.join().unwrap();
}
