//! A deliberately small HTTP/1.1 server on `std::net`: `GET /metrics`
//! returns the exposition text, every other request gets 404.
//!
//! One short-lived thread per connection (capped), socket timeouts on both
//! directions, a bounded request head and `Connection: close` on every
//! response, so a slow or hostile client can hold at most one slot for a few
//! seconds and never blocks other scrapes.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_CONNECTIONS: usize = 16;
const MAX_REQUEST_HEAD: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub type Render = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Metrics,
    NotFound,
}

/// Route a request head (everything up to the blank line).
pub fn route(head: &[u8]) -> Route {
    let line = head.split(|&b| b == b'\n').next().unwrap_or_default();
    let line = std::str::from_utf8(line).unwrap_or_default().trim_end();
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Route::NotFound;
    };
    let path = target.split(['?', '#']).next().unwrap_or_default();
    if method == "GET" && path == "/metrics" && version.starts_with("HTTP/1.") {
        Route::Metrics
    } else {
        Route::NotFound
    }
}

/// Accept connections forever.
pub fn serve(listener: TcpListener, render: Render) -> io::Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) => {
                // EMFILE and friends: back off instead of spinning.
                eprintln!("accept: {e}");
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            drop(stream);
            continue;
        }
        let slot = Slot(active.clone());
        let render = render.clone();
        let spawned = thread::Builder::new().name("http".into()).spawn(move || {
            let _slot = slot;
            let _ = handle(stream, &*render);
        });
        if let Err(e) = spawned {
            eprintln!("spawn connection thread: {e}");
        }
    }
}

/// Releases a connection slot when the handler thread ends.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(head);
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") || head.windows(2).any(|w| w == b"\n\n") {
            return Ok(head);
        }
        if head.len() > MAX_REQUEST_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
    }
}

fn handle(mut stream: TcpStream, render: &(dyn Fn() -> String + Send + Sync)) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let head = read_head(&mut stream)?;
    let (status, content_type, body) = match route(&head) {
        Route::Metrics => ("200 OK", CONTENT_TYPE, render()),
        Route::NotFound => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_owned(),
        ),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    #[test]
    fn routes_only_get_metrics() {
        assert_eq!(
            route(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n"),
            Route::Metrics
        );
        assert_eq!(route(b"GET /metrics?x=1 HTTP/1.0\r\n\r\n"), Route::Metrics);
        assert_eq!(route(b"GET /metrics HTTP/1.1\n\n"), Route::Metrics);
        for head in [
            &b"GET / HTTP/1.1\r\n\r\n"[..],
            b"GET /metrics/ HTTP/1.1\r\n\r\n",
            b"GET /metricsx HTTP/1.1\r\n\r\n",
            b"POST /metrics HTTP/1.1\r\n\r\n",
            b"HEAD /metrics HTTP/1.1\r\n\r\n",
            b"GET /metrics\r\n\r\n",
            b"GET  /metrics HTTP/1.1\r\n\r\n",
            b"\xff\xfe",
            b"",
        ] {
            assert_eq!(
                route(head),
                Route::NotFound,
                "{:?}",
                String::from_utf8_lossy(head)
            );
        }
    }

    fn get(addr: std::net::SocketAddr, request: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_metrics_and_404_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || serve(listener, Arc::new(|| "up 1\n".to_owned())));

        // A client that connects and sends nothing must not block others.
        let _idle = TcpStream::connect(addr).unwrap();

        let ok = get(
            addr,
            "GET /metrics HTTP/1.1\r\nHost: t\r\nAccept-Encoding: gzip\r\n\r\n",
        );
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(ok.contains(&format!("Content-Type: {CONTENT_TYPE}\r\n")));
        assert!(ok.contains("Content-Length: 5\r\n"));
        assert!(ok.ends_with("\r\n\r\nup 1\n"));

        let missing = get(addr, "GET /healthz HTTP/1.1\r\nHost: t\r\n\r\n");
        assert!(
            missing.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{missing}"
        );
    }
}
