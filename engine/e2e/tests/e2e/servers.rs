//! The world's servers, which the engine reaches over the network: an HTTPS proxy that answers for PyPI. It serves
//! from threads of the test process on a free port of 127.0.0.1, reads
//! what to answer from the world's script, and logs each request it gets in the world's ledger, by the latest
//! invocation, the one whose requests these are in every scenario.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use octoflow_e2e::ledger::Ledger;
use octoflow_e2e::{object_at, procs};
use serde_json::Value;
use octoflow_engine::json::Object;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// The file, in the world's directory, of the certificate authority that signed PyPI's certificate, which the engine
/// trusts by SSL_CERT_FILE.
pub const PYPI_CA: &str = "pypi-ca.pem";

/// A world's PyPI proxy, serving until dropped.
pub struct Servers {
    /// The proxy's URL, as https_proxy names it.
    pub proxy_url: String,
    /// The proxy's address, which a drop connects to, so that its accept loop sees `stopping` set.
    proxy_address: std::net::SocketAddr,
    /// true once the servers are being dropped: the proxy accepts no more connections.
    stopping: Arc<AtomicBool>,
    /// Whether the answer to a request has panicked.
    failed: ServerFailure,
    /// The proxy's accept loop.
    threads: Vec<JoinHandle<()>>,
}

impl Servers {
    /// Start the proxy for the world in directory world, after making PyPI's certificate there with openssl. Panics,
    /// failing the scenario, when openssl is missing or fails, or a server cannot listen.
    pub fn start(world: &Path) -> Servers {
        let tls = Arc::new(pypi_tls(world));
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("serving the PyPI proxy: {e}"));
        let proxy_address = proxy.local_addr().unwrap_or_else(|e| panic!("the PyPI proxy's address: {e}"));
        let (stopping, failed) = (Arc::new(AtomicBool::new(false)), ServerFailure(Arc::new(AtomicBool::new(false))));
        let threads = vec![{
            let (world, stopping, failed) = (world.to_path_buf(), Arc::clone(&stopping), Arc::clone(&failed.0));
            std::thread::spawn(move || proxy_accept(&proxy, &world, &tls, &stopping, &failed))
        }];
        let proxy_url = format!("http://{proxy_address}");
        Servers { proxy_url, proxy_address, stopping, failed, threads }
    }

    /// Whether an answer of these servers has panicked, for what waits on the engine to check (ServerFailure::check).
    pub fn failure(&self) -> ServerFailure {
        self.failed.clone()
    }
}

/// Whether the answer of a world's server to a request has panicked: that request went unanswered, or answered 500,
/// and the panic's message is on standard error; the servers go on answering the next ones. The World checks it
/// before it records and while it waits on the engine, so that the panic, not what it made the engine do, is what the
/// scenario fails with.
#[derive(Clone)]
pub struct ServerFailure(Arc<AtomicBool>);

impl ServerFailure {
    /// Panics, failing the scenario at its cause, when an answer has panicked.
    pub fn check(&self) {
        assert!(!self.0.load(Ordering::SeqCst), "a server of the world panicked while answering; its message is above");
    }
}

/// Run answer, setting failed when it panics rather than letting the panic end the server's thread.
fn flag_panic(failed: &AtomicBool, answer: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(answer)).is_err() {
        failed.store(true, Ordering::SeqCst);
    }
}

impl Drop for Servers {
    /// Stops the proxy and waits for its threads, whose answers' panics flag_panic caught.
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the proxy's accept, which then sees stopping; a failure means it no longer listens.
        let _ = TcpStream::connect(self.proxy_address);
        for thread in self.threads.drain(..) {
            // Only a panic of the accept loops themselves, outside any answer, which none of their code raises.
            if thread.join().is_err() {
                eprintln!("a server of the world stopped by a panic");
            }
        }
    }
}

/// Log event, a request a server got, in the world's ledger, by the latest invocation, "0" before the first, with
/// that invocation's processes as the ones whose run states count (Ledger::log).
fn log_request(ledger: &Ledger, event: Object) {
    ledger.change(|state| {
        let latest = object_at(state, "engines")
            .iter()
            .filter_map(|(n, pid)| Some((n.parse::<usize>().ok()?, pid.as_i64()?)))
            .max();
        let (n, pids) = match latest {
            Some((n, pid)) => {
                let pid = i32::try_from(pid).expect("a pid fits an i32");
                (n, procs::descendants(pid).unwrap_or_else(|e| panic!("listing invocation {n}'s processes: {e}")))
            }
            None => (0, BTreeSet::new()),
        };
        ledger.log(state, event, &n.to_string(), &pids);
    });
}

// -- PyPI, behind its proxy ---------------------------------------------------------

/// text with each `%XX` decoded to its byte, X a hex digit, the bytes read as UTF-8 with replacement; a `%`
/// before anything else stays as it is.
///
/// Not the engine's `serve::url::unquote_plus`: the fake servers read the engine's requests with code of their own,
/// so that a fault the engine's URL code shares between writing and reading still shows.
fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Two hex digits, not a number from_str_radix would take, which may start with a sign.
        let hex = bytes
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The TLS configuration of pypi.org's server: a certificate for pypi.org signed by a certificate authority of the
/// world's own, PYPI_CA, made with openssl in world. A leaf under a CA, as PyPI's is, since rustls refuses a server
/// certificate that is itself a CA's, as a self-signed one is (CaUsedAsEndEntity).
fn pypi_tls(world: &Path) -> rustls::ServerConfig {
    let path = |name: &str| world.join(name).to_string_lossy().into_owned();
    let key = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "30"];
    let (ca, ca_key, cert, cert_key) = (path(PYPI_CA), path("pypi-ca-key.pem"), path("pypi-cert.pem"), path("pypi-key.pem"));
    let ca_args = ["-subj", "/CN=Parity CA", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign"];
    let leaf_args = ["-subj", "/CN=pypi.org", "-addext", "subjectAltName=DNS:pypi.org", "-addext", "basicConstraints=critical,CA:FALSE"];
    openssl(&[&["req", "-x509"], &key[..], &["-keyout", &ca_key, "-out", &ca], &ca_args[..]].concat());
    openssl(&[&["req", "-x509"], &key[..], &["-CA", &ca, "-CAkey", &ca_key, "-keyout", &cert_key, "-out", &cert], &leaf_args[..]].concat());
    let chain = CertificateDer::pem_file_iter(&cert)
        .unwrap_or_else(|e| panic!("reading {cert}: {e}"))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("reading {cert}: {e}"));
    let private = PrivateKeyDer::from_pem_file(&cert_key).unwrap_or_else(|e| panic!("reading {cert_key}: {e}"));
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_no_client_auth()
        .with_single_cert(chain, private)
        .unwrap_or_else(|e| panic!("PyPI's certificate: {e}"))
}

/// Run openssl with args; panics, naming it, where it cannot run or fails.
fn openssl(args: &[&str]) {
    let out = std::process::Command::new("openssl")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("running openssl, which makes the fake PyPI's certificate: {e}"));
    assert!(
        out.status.success(),
        "openssl could not make the fake PyPI's certificate, {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
}

/// Accept the proxy's connections until stopping, each answered on a thread of its own.
fn proxy_accept(listener: &TcpListener, world: &Path, tls: &Arc<rustls::ServerConfig>, stopping: &AtomicBool, failed: &Arc<AtomicBool>) {
    let mut connections = Vec::new();
    for stream in listener.incoming() {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let (world, tls, failed): (PathBuf, _, _) = (world.to_path_buf(), Arc::clone(tls), Arc::clone(failed));
        connections.push(std::thread::spawn(move || flag_panic(&failed, || proxy_connection(stream, &world, tls))));
    }
    for connection in connections {
        // flag_panic caught each connection's panic.
        let _ = connection.join();
    }
}

/// One connection to the proxy: a CONNECT to pypi.org:443, answered by PyPI over TLS with the world's certificate;
/// any other target is refused with 403. A client that goes away midway ends it quietly.
fn proxy_connection(mut stream: TcpStream, world: &Path, tls: Arc<rustls::ServerConfig>) {
    let Some(head) = read_head(&mut stream) else { return };
    let target = head.split(' ').nth(1).unwrap_or_default();
    if !head.starts_with("CONNECT ") || target != "pypi.org:443" {
        let _ = stream.write_all(b"HTTP/1.0 403 the parity proxy reaches only pypi.org\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    if stream.write_all(b"HTTP/1.0 200 OK\r\n\r\n").is_err() {
        return;
    }
    let connection = rustls::ServerConnection::new(tls).unwrap_or_else(|e| panic!("PyPI's TLS: {e}"));
    let mut tls_stream = rustls::StreamOwned::new(connection, stream);
    let Some(head) = read_head(&mut tls_stream) else { return };
    let path = head.split(' ').nth(1).unwrap_or_default().to_string();
    log_request(&Ledger::new(world), octoflow_engine::json::object([("pypi", Value::from(format!("GET {path}")))]));
    let name = path.strip_prefix("/pypi/").map(|rest| unquote(rest.split('/').next().unwrap_or_default()));
    let answer = name.and_then(|name| Ledger::new(world).script().get("pypi").and_then(|p| p.get(&name)).cloned());
    let (status, body) = match answer {
        Some(answer) => ("200 OK", answer.to_string()),
        None => ("404 Not Found", "not found".to_string()),
    };
    let response = format!("HTTP/1.0 {status}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    if tls_stream.write_all(response.as_bytes()).and_then(|()| tls_stream.flush()).is_ok() {
        tls_stream.conn.send_close_notify();
        let _ = tls_stream.flush();
    }
}

/// An HTTP request's head, its request line and headers, read byte by byte up to the blank line that ends it, so that
/// nothing after it is consumed; None when the client goes away first.
fn read_head(stream: &mut impl Read) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

#[test]
fn unquote_decodes_each_escape_and_keeps_a_stray_percent() {
    assert_eq!(unquote("100%25%zz%e2%80%94"), "100%%zz\u{2014}");
    assert_eq!(unquote("a%+1%-1%"), "a%+1%-1%");
}

#[test]
fn a_panicking_answer_is_flagged_and_the_next_one_still_runs() {
    let failed = AtomicBool::new(false);
    flag_panic(&failed, || {});
    assert!(!failed.load(Ordering::SeqCst));
    flag_panic(&failed, || panic!("an answer that panics"));
    let mut ran = false;
    flag_panic(&failed, || ran = true);
    assert!(failed.load(Ordering::SeqCst) && ran);
}
