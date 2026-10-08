//! Minimal CONNECT-only egress: the model transport is allowed, arbitrary browsing is not.
//! No headers, credentials, prompts or payloads are logged by this proxy.
use crate::{Result, ensure};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixListener;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

pub(super) fn host(host: &str) -> Result<()> {
    ensure(
        host.len() <= 253
            && host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
            && host
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
            && host.parse::<std::net::IpAddr>().is_err(),
        "API hosts must be exact lowercase DNS names, not IPs or patterns",
    )
}
fn authority(header: &str, allowed: &[String]) -> Result<String> {
    let mut line = header
        .lines()
        .next()
        .ok_or("missing CONNECT line")?
        .split_whitespace();
    ensure(
        line.next() == Some("CONNECT"),
        "only HTTPS CONNECT is supported",
    )?;
    let target = line.next().ok_or("missing target")?;
    ensure(
        matches!(line.next(), Some("HTTP/1.1" | "HTTP/1.0")) && line.next().is_none(),
        "invalid CONNECT line",
    )?;
    let hostname = target
        .strip_suffix(":443")
        .ok_or("only port 443 is allowed")?;
    host(hostname)?;
    ensure(
        allowed.iter().any(|h| h == hostname),
        "API host is not approved",
    )?;
    Ok(hostname.into())
}
enum Socket {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}
impl Socket {
    fn duplicate(&self) -> std::io::Result<Self> {
        match self {
            Self::Tcp(s) => Ok(Self::Tcp(s.try_clone()?)),
            #[cfg(unix)]
            Self::Unix(s) => Ok(Self::Unix(s.try_clone()?)),
        }
    }
    fn close(&self) {
        match self {
            Self::Tcp(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
            #[cfg(unix)]
            Self::Unix(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
    fn timeout(&self) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => {
                s.set_read_timeout(Some(Duration::from_secs(300)))?;
                s.set_write_timeout(Some(Duration::from_secs(30)))
            }
            #[cfg(unix)]
            Self::Unix(s) => {
                s.set_read_timeout(Some(Duration::from_secs(300)))?;
                s.set_write_timeout(Some(Duration::from_secs(30)))
            }
        }
    }
}
impl Read for Socket {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.read(b),
            #[cfg(unix)]
            Self::Unix(s) => s.read(b),
        }
    }
}
impl Write for Socket {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.write(b),
            #[cfg(unix)]
            Self::Unix(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Self::Unix(s) => s.flush(),
        }
    }
}
type Connections = Arc<Mutex<BTreeMap<u64, Vec<Socket>>>>;
fn tunnel(mut left: Socket, mut right: Socket, id: u64, active: &Connections) -> Result<()> {
    left.timeout()?;
    right.timeout()?;
    active
        .lock()
        .map_err(|_| "proxy lock poisoned")?
        .insert(id, vec![left.duplicate()?, right.duplicate()?]);
    let mut a = left.duplicate()?;
    let mut b = right.duplicate()?;
    let worker = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a, &mut b);
        b.close();
        a.close();
    });
    let _ = std::io::copy(&mut right, &mut left);
    left.close();
    right.close();
    let _ = worker.join();
    active
        .lock()
        .map_err(|_| "proxy lock poisoned")?
        .remove(&id);
    Ok(())
}
fn forbidden_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
        }
        std::net::IpAddr::V6(ip) => {
            ip.to_ipv4_mapped()
                .is_some_and(|v4| forbidden_address(std::net::IpAddr::V4(v4)))
                || ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

fn serve(mut client: Socket, allowed: &[String], id: u64, active: &Connections) -> Result<()> {
    client.timeout()?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        ensure(
            client.read(&mut b)? == 1 && header.len() < 8192,
            "invalid proxy header",
        )?;
        header.push(b[0]);
    }
    let target = authority(std::str::from_utf8(&header)?, allowed)?;
    let addresses = (target.as_str(), 443).to_socket_addrs()?;
    let mut connected = None;
    for address in addresses {
        if !forbidden_address(address.ip())
            && let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        {
            connected = Some(stream);
            break;
        }
    }
    let server = connected.ok_or("cannot connect to approved public API endpoint")?;
    client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
    tunnel(client, Socket::Tcp(server), id, active)
}

pub(super) struct Proxy {
    pub port: u16,
    /// The Unix socket bubblewrap exposes inside the private network namespace.
    #[cfg(target_os = "linux")]
    pub socket: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    active: Connections,
    worker: Option<std::thread::JoinHandle<()>>,
    // Removed with the socket after `drop` has joined the listener thread.
    #[cfg(target_os = "linux")]
    _socket_dir: tempfile::TempDir,
}
impl Proxy {
    pub fn start(allowed: &[String]) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        // A fresh 0700 directory directly under /tmp: `sun_path` holds at most 108 bytes,
        // and stage directories can be nested arbitrarily deep.
        #[cfg(target_os = "linux")]
        let (socket_dir, socket, unix) = {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::Builder::new()
                .prefix("kb-eval-")
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in("/tmp")?;
            let socket = dir.path().join("egress.sock");
            let u = UnixListener::bind(&socket)?;
            u.set_nonblocking(true)?;
            (dir, socket, u)
        };
        let stop = Arc::new(AtomicBool::new(false));
        let active: Connections = Default::default();
        let st = stop.clone();
        let ac = active.clone();
        let allowed = allowed.to_vec();
        let worker = std::thread::spawn(move || {
            let ids = AtomicU64::new(0);
            while !st.load(Ordering::Acquire) {
                let mut clients = Vec::new();
                // BSD accept(2) copies O_NONBLOCK from the polled listener; client I/O must
                // block (bounded by `Socket::timeout`) or reads fail with WouldBlock.
                if let Ok((c, _)) = listener.accept()
                    && c.set_nonblocking(false).is_ok()
                {
                    clients.push(Socket::Tcp(c));
                }
                #[cfg(target_os = "linux")]
                if let Ok((c, _)) = unix.accept()
                    && c.set_nonblocking(false).is_ok()
                {
                    clients.push(Socket::Unix(c));
                }
                for client in clients {
                    if ac.lock().is_ok_and(|map| map.len() >= 32) {
                        client.close();
                        continue;
                    }
                    let active = ac.clone();
                    let approved = allowed.clone();
                    let id = ids.fetch_add(1, Ordering::Relaxed);
                    if let Ok(copy) = client.duplicate()
                        && let Ok(mut map) = active.lock()
                    {
                        map.insert(id, vec![copy]);
                    }
                    std::thread::spawn(move || {
                        let _ = serve(client, &approved, id, &active);
                        if let Ok(mut map) = active.lock()
                            && let Some(sockets) = map.remove(&id)
                        {
                            for s in sockets {
                                s.close();
                            }
                        }
                    });
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        Ok(Self {
            port,
            #[cfg(target_os = "linux")]
            socket,
            stop,
            active,
            worker: Some(worker),
            #[cfg(target_os = "linux")]
            _socket_dir: socket_dir,
        })
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Ok(mut active) = self.active.lock() {
            for (_, sockets) in std::mem::take(&mut *active) {
                for s in sockets {
                    s.close();
                }
            }
        }
    }
}

pub(super) fn bridge(socket: &Path, program: &Path, args: &[String]) -> Result<i32> {
    #[cfg(unix)]
    {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let path = socket.to_path_buf();
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                if let Ok(remote) = UnixStream::connect(&path) {
                    std::thread::spawn(move || {
                        let _ = tunnel(
                            Socket::Tcp(client),
                            Socket::Unix(remote),
                            0,
                            &Default::default(),
                        );
                    });
                }
            }
        });
        let mut c = std::process::Command::new(program);
        c.args(args);
        let url = format!("http://127.0.0.1:{port}");
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            c.env(name, &url);
        }
        Ok(c.status()?.code().unwrap_or(1))
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, program, args);
        Err("Unix proxy bridge unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapped_ipv4_does_not_bypass_private_destination_checks() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.1.1",
        ] {
            assert!(
                forbidden_address(ip.parse().unwrap()),
                "private destination allowed: {ip}"
            );
        }
        assert!(!forbidden_address("8.8.8.8".parse().unwrap()));
        assert!(!forbidden_address("::ffff:8.8.8.8".parse().unwrap()));
    }
    #[test]
    fn only_exact_https_api_targets_are_allowed() {
        let hosts = vec!["api.example.invalid".into()];
        assert_eq!(
            authority("CONNECT api.example.invalid:443 HTTP/1.1\r\n\r\n", &hosts).unwrap(),
            "api.example.invalid"
        );
        for h in [
            "GET https://api.example.invalid/ HTTP/1.1",
            "CONNECT api.example.invalid:80 HTTP/1.1",
            "CONNECT user@api.example.invalid:443 HTTP/1.1",
            "CONNECT other.example.invalid:443 HTTP/1.1",
            "CONNECT 127.0.0.1:443 HTTP/1.1",
        ] {
            assert!(authority(h, &hosts).is_err());
        }
    }
    #[test]
    fn accepted_clients_wait_for_their_request() {
        let proxy = Proxy::start(&["api.example.invalid".into()]).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
        // Stay idle past the proxy's accept: a non-blocking accepted socket fails its first
        // read and is closed instead of waiting for the request.
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let idle = client.read(&mut [0; 64]);
        assert!(
            idle.as_ref().is_err_and(|e| matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )),
            "proxy closed a client before its request arrived: {idle:?}"
        );
        client
            .write_all(b"CONNECT other.example.invalid:443 HTTP/1.1\r\n\r\n")
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut reply = Vec::new();
        let _ = client.read_to_end(&mut reply);
        assert!(reply.is_empty(), "an unapproved target received a reply");
    }
}
