use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use pathon_core::{FrameReport, Renderer};

use crate::capture::snapshot;
use crate::protocol::Snapshot;

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:7425";

// A slow or vanished client must never stall the render thread.
const WRITE_TIMEOUT: Duration = Duration::from_millis(20);

fn lock(clients: &Mutex<Vec<TcpStream>>) -> MutexGuard<'_, Vec<TcpStream>> {
    clients
        .lock()
        .expect("the client list is only pushed to and filtered, which do not panic")
}

// The renderer side: accepts devtools clients on localhost and streams snapshots to them, at most `max_rate` times a
// second so capturing costs nothing measurable between publishes.
#[derive(Debug)]
pub struct DevtoolsServer {
    address: SocketAddr,
    clients: Arc<Mutex<Vec<TcpStream>>>,
    interval: Duration,
    last_publish: Option<Instant>,
    last_frame: Option<Instant>,
    frame: u64,
    capturing: bool,
}

impl DevtoolsServer {
    pub fn start(address: impl ToSocketAddrs) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        let clients = Arc::new(Mutex::new(Vec::new()));
        let accepted = Arc::clone(&clients);
        thread::Builder::new()
            .name("pathon-devtools-accept".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_ok() {
                        let _ = stream.set_nodelay(true);
                        lock(&accepted).push(stream);
                    }
                }
            })?;
        log::info!("pathon devtools listening on {address}");
        Ok(Self {
            address,
            clients,
            interval: Duration::from_millis(100),
            last_publish: None,
            last_frame: None,
            frame: 0,
            capturing: false,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    #[must_use]
    pub fn with_max_rate(self, per_second: u32) -> Self {
        Self {
            interval: Duration::from_secs(1) / per_second.max(1),
            ..self
        }
    }

    pub fn client_count(&self) -> usize {
        lock(&self.clients).len()
    }

    pub fn publish(&self, snapshot: &Snapshot) {
        let mut line = snapshot.to_line();
        line.push('\n');
        lock(&self.clients).retain_mut(|client| client.write_all(line.as_bytes()).is_ok());
    }

    // Call after every `render_frame`. Capture is on only while a client listens, so it applies from the next
    // frame; `report` is published only when it was rendered with capture on.
    pub fn frame(&mut self, renderer: &mut Renderer, report: &FrameReport) {
        let now = Instant::now();
        #[expect(clippy::cast_possible_truncation, reason = "a frame time in milliseconds fits f32")]
        let frame_ms = self.last_frame.map_or(0.0, |last| (now - last).as_secs_f64() * 1000.0) as f32;
        self.last_frame = Some(now);
        self.frame += 1;
        let report_captured = self.capturing;
        self.capturing = self.client_count() > 0;
        renderer.set_capture(self.capturing);
        let due = self.last_publish.is_none_or(|last| now - last >= self.interval);
        if report_captured && self.capturing && due {
            self.publish(&snapshot(self.frame, frame_ms, renderer, report));
            self.last_publish = Some(now);
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ClientEvent {
    Connected(String),
    Disconnected(String),
    Snapshot(Box<Snapshot>),
}

// The devtools side: keeps reconnecting to `address` and forwards every snapshot it reads.
pub fn connect(address: String, events: Sender<ClientEvent>) {
    let spawned = thread::Builder::new()
        .name("pathon-devtools-client".into())
        .spawn(move || {
            loop {
                match TcpStream::connect(&address) {
                    Ok(stream) => {
                        if events.send(ClientEvent::Connected(address.clone())).is_err() {
                            return;
                        }
                        for line in BufReader::new(stream).lines() {
                            let Ok(line) = line else {
                                break;
                            };
                            if let Ok(snapshot) = Snapshot::from_line(&line)
                                && events.send(ClientEvent::Snapshot(Box::new(snapshot))).is_err()
                            {
                                return;
                            }
                        }
                        if events.send(ClientEvent::Disconnected(address.clone())).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        if events.send(ClientEvent::Disconnected(address.clone())).is_err() {
                            return;
                        }
                    }
                }
                thread::sleep(Duration::from_secs(1));
            }
        });
    if let Err(error) = spawned {
        log::error!("cannot start the devtools client thread: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_renderer;

    #[test]
    fn a_connected_client_receives_only_captured_frames() {
        let mut server = DevtoolsServer::start("127.0.0.1:0").expect("bind a free port");
        let (mut renderer, input) = demo_renderer();
        let client = TcpStream::connect(server.address()).expect("connect");
        while server.client_count() == 0 {
            thread::yield_now();
        }
        for _ in 0..2 {
            let report = renderer.render_frame(&input).expect("render");
            server.frame(&mut renderer, &report);
        }
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).expect("read a snapshot");
        let snapshot = Snapshot::from_line(line.trim()).expect("parse");

        assert_eq!(snapshot.frame, 2);
        assert_eq!(snapshot.views.len(), 2);
        assert!(snapshot.views[0].passes.len() > 1, "the demo records several passes");
    }
}
