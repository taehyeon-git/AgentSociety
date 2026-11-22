//! Real loopback HTTP fixture shared by protocol, engine, and process tests.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct Request {
    pub path: String,
    pub body: Value,
    pub authorization: Option<String>,
}

impl Request {
    pub fn context(&self) -> Value {
        serde_json::from_str(self.body["messages"][1]["content"].as_str().unwrap()).unwrap()
    }
}

pub struct Response {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: value.to_string(),
            delay: Duration::ZERO,
            headers: Vec::new(),
        }
    }
    pub fn ollama(content: &str) -> Self {
        Self::json(
            json!({"message":{"role":"assistant","content":content}, "done":true, "done_reason":"stop", "prompt_eval_count":100,"eval_count":20}),
        )
    }
    pub fn openai(content: &str) -> Self {
        Self::json(
            json!({"choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":20}}),
        )
    }
    pub fn rest() -> Self {
        Self::ollama(r#"{"action":{"action":"rest"},"reasoning_summary":"Recover energy."}"#)
    }
}

pub struct Server {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    peak: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn new(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let (log, max_active, stopped) =
            (Arc::clone(&requests), Arc::clone(&peak), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (log, active, max_active, handler) = (
                            Arc::clone(&log),
                            Arc::clone(&active),
                            Arc::clone(&max_active),
                            Arc::clone(&handler),
                        );
                        workers.push(thread::spawn(move || {
                            let Some(request) = read_request(&mut stream) else { return; };
                            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max_active.fetch_max(count, Ordering::SeqCst);
                            log.lock().unwrap().push(request.clone());
                            let response = handler(&request);
                            thread::sleep(response.delay);
                            // Timed-out clients may have closed their socket; this is expected.
                            let mut headers = format!("HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", response.status, response.body.len());
                            for (name, value) in response.headers { headers.push_str(&format!("{name}: {value}\r\n")); }
                            headers.push_str("\r\n");
                            let _ = stream.write_all(headers.as_bytes());
                            let _ = stream.write_all(response.body.as_bytes());
                            let _ = stream.flush();
                            active.fetch_sub(1, Ordering::SeqCst);
                            // Half-close before dropping so a client rejecting a large body
                            // sees the HTTP headers instead of a Windows TCP reset race.
                            let _ = stream.shutdown(std::net::Shutdown::Write);
                            let mut closed = [0_u8; 1];
                            let _ = stream.read(&mut closed);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("mock accept failed: {error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            base_url,
            requests,
            peak,
            stop,
            thread: Some(thread),
        }
    }

    pub fn peak_concurrency(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    // Windows may inherit the listener's nonblocking mode on accepted sockets.
    // A request body arriving just after accept must wait, not close as WouldBlock.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() < 131_072, "test request unexpectedly large");
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length: usize = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .trim()
        .parse()
        .unwrap();
    while bytes.len() < header_end + length {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Some(Request {
        path: headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .into(),
        authorization: headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.trim().into()),
        body: serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
    })
}
