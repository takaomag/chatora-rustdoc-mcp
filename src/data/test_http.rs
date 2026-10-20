//! A local HTTP server for the tests of the data layer.

use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::TcpListener,
    sync::mpsc,
    thread,
};

/// Serves one HTTP response on a local port, and sends back the request head it received.
pub(super) fn serve_once(head: &str, body: &[u8]) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
    let base_url = format!("http://{}", listener.local_addr().expect("bound"));
    let (sender, request) = mpsc::channel();
    let (head, body) = (head.to_owned(), body.to_vec());
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("the client connects");
        let mut reader = BufReader::new(stream);
        let mut request_head = String::new();
        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .expect("the request is readable");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            request_head.push_str(&line);
        }
        let mut stream = reader.into_inner();
        stream
            .write_all(closing(&head).as_bytes())
            .expect("the response is writable");
        stream.write_all(&body).expect("the response is writable");
        sender.send(request_head).expect("the test waits");
    });
    (base_url, request)
}

/// Serves `body` with 200 to every request on a local port, each after `delay`, and counts the
/// requests: for the tests of calls that come while a fetch is under way.
pub(super) fn serve_slowly(
    body: &[u8],
    delay: std::time::Duration,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
    let base_url = format!("http://{}", listener.local_addr().expect("bound"));
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&requests);
    let body = body.to_vec();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            counted.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                    line.clear();
                }
                thread::sleep(delay);
                let mut stream = reader.into_inner();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                // What the client makes of it is what the test looks at.
                let _written = stream
                    .write_all(head.as_bytes())
                    .and_then(|()| stream.write_all(&body));
            });
        }
    });
    (base_url, requests)
}

/// Serves `responses` (a head and a body each) in turn on a local port, one connection each, and
/// sends back the request heads it received.
pub(super) fn serve_sequence(
    responses: Vec<(String, Vec<u8>)>,
) -> (String, mpsc::Receiver<String>) {
    serve_sequence_with(|_| responses)
}

/// [`serve_sequence`] with responses made from the base URL of the server, for bodies that name
/// URLs on it.
pub(super) fn serve_sequence_with(
    responses: impl FnOnce(&str) -> Vec<(String, Vec<u8>)>,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
    let base_url = format!("http://{}", listener.local_addr().expect("bound"));
    let responses = responses(&base_url);
    let (sender, requests) = mpsc::channel();
    thread::spawn(move || {
        for (head, body) in responses {
            let (stream, _) = listener.accept().expect("the client connects");
            let mut reader = BufReader::new(stream);
            let mut request_head = String::new();
            loop {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .expect("the request is readable");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                request_head.push_str(&line);
            }
            let mut stream = reader.into_inner();
            stream
                .write_all(closing(&head).as_bytes())
                .expect("the response is writable");
            stream.write_all(&body).expect("the response is writable");
            if sender.send(request_head).is_err() {
                return;
            }
        }
    });
    (base_url, requests)
}

/// `head` with `Connection: close` after its status line. The server answers one request on each
/// connection and then closes it. Without the header, the client may send its next request on the
/// same connection, and closing a connection with that request unread resets it, so that the
/// client fails with `ECONNRESET` now and then.
fn closing(head: &str) -> String {
    let (status, fields) = head.split_once("\r\n").expect("a status line");
    format!("{status}\r\nConnection: close\r\n{fields}")
}
