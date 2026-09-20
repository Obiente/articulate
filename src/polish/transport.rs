//! Cancellable HTTP transport. ureq's body timeout is a total budget, not an idle timeout.
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use ureq::unversioned::{
    resolver::DefaultResolver,
    transport::{
        Buffers, ConnectProxyConnector, ConnectionDetails, Connector, Either, LazyBuffers,
        NextTimeout, RustlsConnector, Transport,
    },
};

#[derive(Debug, Clone)]
struct CancellableTcp {
    cancel: Arc<AtomicBool>,
    idle: Duration,
}
struct Socket {
    stream: TcpStream,
    buffers: LazyBuffers,
    cancel: Arc<AtomicBool>,
    idle: Duration,
}
impl std::fmt::Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CancellableDownloadSocket")
    }
}
fn cancelled(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Download cancelled",
        ))
    } else {
        Ok(())
    }
}
impl<In: Transport> Connector<In> for CancellableTcp {
    type Out = Either<In, Socket>;
    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        if let Some(existing) = chained {
            return Ok(Some(Either::A(existing)));
        }
        let start = Instant::now();
        let budget = details
            .timeout
            .not_zero()
            .map(|d| *d)
            .unwrap_or(Duration::from_secs(15))
            .min(Duration::from_secs(15));
        let mut last = io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "No download address was reachable",
        );
        // Short connect attempts keep cancellation responsive, including an unreachable address.
        while start.elapsed() < budget {
            for addr in &details.addrs {
                cancelled(&self.cancel)?;
                let remaining = budget.saturating_sub(start.elapsed());
                if remaining.is_zero() {
                    break;
                }
                match TcpStream::connect_timeout(addr, remaining.min(Duration::from_millis(500))) {
                    Ok(stream) => {
                        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
                        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                        stream.set_nodelay(true)?;
                        return Ok(Some(Either::B(Socket {
                            stream,
                            buffers: LazyBuffers::new(
                                details.config.input_buffer_size(),
                                details.config.output_buffer_size(),
                            ),
                            cancel: self.cancel.clone(),
                            idle: self.idle,
                        })));
                    }
                    Err(e) => last = e,
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(last.into())
    }
}
impl Transport for Socket {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        let start = Instant::now();
        let mut last_progress = start;
        let mut sent = 0;
        while sent < amount {
            cancelled(&self.cancel)?;
            if start.elapsed() >= *timeout.after {
                return Err(ureq::Error::Timeout(timeout.reason));
            }
            if last_progress.elapsed() >= self.idle {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Download connection stopped sending",
                )
                .into());
            }
            match self.stream.write(&self.buffers.output()[sent..amount]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Download connection closed",
                    )
                    .into());
                }
                Ok(n) => {
                    sent += n;
                    last_progress = Instant::now();
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let start = Instant::now();
        loop {
            cancelled(&self.cancel)?;
            if start.elapsed() >= *timeout.after {
                return Err(ureq::Error::Timeout(timeout.reason));
            }
            if start.elapsed() >= self.idle {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Download connection stopped receiving",
                )
                .into());
            }
            match self.stream.read(self.buffers.input_append_buf()) {
                Ok(n) => {
                    self.buffers.input_appended(n);
                    return Ok(n > 0);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    // Downloads use one response per connection; do not retain pooled sockets.
    fn is_open(&mut self) -> bool {
        false
    }
}
pub(super) fn agent(cancel: Arc<AtomicBool>, idle: Duration, https_only: bool) -> ureq::Agent {
    let connector =
        ().chain(ConnectProxyConnector::default())
            .chain(CancellableTcp { cancel, idle })
            .chain(RustlsConnector::default());
    let config = ureq::Agent::config_builder()
        .https_only(https_only)
        .timeout_global(Some(Duration::from_secs(3600)))
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(None)
        .http_status_as_error(false)
        .build();
    ureq::Agent::with_parts(config, connector, DefaultResolver::default())
}
