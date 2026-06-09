//! Non-blocking TCP reachability probing.
//!
//! A small pool of worker threads drains a shared job queue and reports results
//! over an `mpsc` channel. The UI thread polls results each loop iteration via
//! [`LivenessProbe::poll`] and never blocks on the network.

use std::collections::VecDeque;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Liveness {
    #[default]
    Unknown,
    Checking,
    Up,
    Down,
    /// Behind a proxy (ProxyJump/ProxyCommand) — a direct TCP probe is
    /// meaningless, so we skip it.
    Skipped,
}

impl Liveness {
    pub fn glyph(self) -> &'static str {
        match self {
            Liveness::Unknown => "·",
            Liveness::Checking => "…",
            Liveness::Up => "●",
            Liveness::Down => "○",
            Liveness::Skipped => "—",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LivenessResult {
    /// Index of the host in `App::hosts` this result belongs to.
    pub id: usize,
    pub state: Liveness,
    pub rtt: Option<Duration>,
}

/// One probe job.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub id: usize,
    pub target: String,
    pub port: u16,
    pub has_jump: bool,
}

/// A live probing session. Dropping it lets the workers finish and exit (the
/// queue drains, then `recv` on the job side returns and threads end).
pub struct LivenessProbe {
    rx: Receiver<LivenessResult>,
    _handles: Vec<JoinHandle<()>>,
}

impl LivenessProbe {
    pub fn spawn(targets: Vec<ProbeTarget>, timeout: Duration, workers: usize) -> Self {
        let queue: Arc<Mutex<VecDeque<ProbeTarget>>> =
            Arc::new(Mutex::new(targets.into_iter().collect()));
        let (tx, rx): (Sender<LivenessResult>, Receiver<LivenessResult>) = mpsc::channel();

        let mut handles = Vec::new();
        let n = workers.max(1);
        for _ in 0..n {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let handle = std::thread::spawn(move || {
                loop {
                    let job = {
                        let mut q = queue.lock().unwrap();
                        q.pop_front()
                    };
                    let Some(job) = job else { break };

                    if job.has_jump {
                        let _ = tx.send(LivenessResult {
                            id: job.id,
                            state: Liveness::Skipped,
                            rtt: None,
                        });
                        continue;
                    }

                    let _ = tx.send(LivenessResult {
                        id: job.id,
                        state: Liveness::Checking,
                        rtt: None,
                    });

                    let (state, rtt) = tcp_probe(&job.target, job.port, timeout);
                    let _ = tx.send(LivenessResult {
                        id: job.id,
                        state,
                        rtt,
                    });
                }
            });
            handles.push(handle);
        }
        // Drop our own sender clone so the channel closes once workers finish.
        drop(tx);

        LivenessProbe {
            rx,
            _handles: handles,
        }
    }

    /// Non-blocking drain of any results that have arrived. The second tuple
    /// element is `true` once the channel has closed (all workers finished),
    /// signalling the caller it can drop this probe.
    pub fn drain(&self) -> (Vec<LivenessResult>, bool) {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(r) => out.push(r),
                Err(TryRecvError::Empty) => return (out, false),
                Err(TryRecvError::Disconnected) => return (out, true),
            }
        }
    }
}

/// Attempt a TCP connection within `timeout`; report Up (with RTT) or Down.
/// DNS resolution happens here, on the worker thread.
pub fn tcp_probe(target: &str, port: u16, timeout: Duration) -> (Liveness, Option<Duration>) {
    let started = Instant::now();
    let addrs = match (target, port).to_socket_addrs() {
        Ok(a) => a,
        Err(_) => return (Liveness::Down, None),
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return (Liveness::Up, Some(started.elapsed()));
        }
    }
    (Liveness::Down, None)
}
