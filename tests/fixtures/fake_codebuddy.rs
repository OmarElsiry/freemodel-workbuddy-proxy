use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

fn capture_environment() {
    let Some(path) = std::env::var_os("WORKBUDDY_PROXY_TEST_ENV_CAPTURE") else {
        return;
    };
    let environment = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    let bytes = serde_json::to_vec(&environment).expect("serialize fake sidecar environment");
    let mut output = OpenOptions::new()
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .expect("open fake sidecar environment capture");
    output
        .write_all(&bytes)
        .expect("write fake sidecar environment capture");
    output
        .set_permissions(fs::Permissions::from_mode(0o600))
        .expect("protect fake sidecar environment capture");
}

fn main() {
    capture_environment();
    let args = std::env::args().collect::<Vec<_>>();
    let port = args
        .windows(2)
        .find(|pair| pair[0] == "--port")
        .and_then(|pair| pair[1].parse::<u16>().ok())
        .expect("--port is required");
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind fake sidecar");
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut request = [0_u8; 2048];
        let size = stream.read(&mut request).unwrap_or_default();
        let request = String::from_utf8_lossy(&request[..size]);
        let status = if request.starts_with("GET /api/v1/health ") {
            "200 OK"
        } else {
            "404 Not Found"
        };
        let body = if status == "200 OK" { "ok" } else { "missing" };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    }
}
