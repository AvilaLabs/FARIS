//! The account client sends its token only to the host that issued it (SEC-042): a mock server on the
//! loopback interface plays the issuing host and a second one plays any other host.
#![cfg(not(target_arch = "wasm32"))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use avila_account::account::{AccountError, Client, normalize_base};

const TOKEN: &str = "mock-token-0123456789";

/// Answers the first request with `response`, and sends the raw request text through the channel.
fn serve_once(listener: TcpListener, response: String) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buffer[..n]),
                }
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
        }
    });
    receiver
}

fn loopback() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (listener, base)
}

// Verifies: SEC-042
#[test]
fn the_token_goes_to_the_issuing_host_and_a_redirect_is_not_followed() {
    let (other_listener, other_base) = loopback();
    other_listener.set_nonblocking(true).unwrap();
    let (issuer_listener, issuer_base) = loopback();
    let redirect = format!(
        "HTTP/1.1 302 Found\r\nLocation: {other_base}/api/auth/me\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let issued = serve_once(issuer_listener, redirect);

    let client = Client::with_token(&issuer_base, TOKEN).unwrap();
    let result = client.whoami();

    // The issuing host got the token, as a bearer header on its own base URL.
    let request = issued.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(request.starts_with("GET /api/auth/me "), "{request}");
    assert!(
        request
            .to_lowercase()
            .contains(&format!("authorization: bearer {TOKEN}").to_lowercase()),
        "{request}"
    );
    // The redirect's empty body is an error reply, not the other host's answer, and the other host never saw
    // a connection.
    assert!(
        matches!(
            result,
            Err(AccountError::Protocol(_) | AccountError::Http { .. })
        ),
        "{result:?}"
    );
    thread::sleep(Duration::from_millis(200));
    assert!(
        other_listener.accept().is_err(),
        "the redirect target was contacted"
    );
    // The client's debug text hides the token.
    assert!(!format!("{client:?}").contains(TOKEN));
}

// Verifies: SEC-042
#[test]
fn plain_http_is_refused_except_for_loopback_and_https_is_accepted() {
    for base in [
        "http://example.com",
        "http://api.avilalabs.org",
        "http://10.0.0.5:8080",
        "http://192.168.1.2",
        "ftp://example.com",
        "api.avilalabs.org",
        "https://user@example.com",
    ] {
        assert!(normalize_base(base).is_err(), "{base}");
        assert!(Client::with_token(base, TOKEN).is_err(), "{base}");
    }
    for base in [
        "http://localhost:8787",
        "http://127.0.0.1:9",
        "http://[::1]:80",
        "https://api.avilalabs.org",
    ] {
        assert!(normalize_base(base).is_ok(), "{base}");
    }
}

// Verifies: SEC-042
#[test]
fn the_https_stack_verifies_certificates_with_a_bundled_root_store() {
    // The client's TLS is rustls with the webpki root store (ureq's "tls" feature); the lock file pins them
    // and nothing here switches certificate checking off.
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    for package in ["rustls", "webpki-roots", "ureq"] {
        assert!(lock.contains(&format!("name = \"{package}\"")), "{package}");
    }
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("dangerous"),
        "no certificate-check bypass feature"
    );
}
