//! Loopback HTTP transport called by the C UI host while its handlers are ported.
//! The host's run thread owns dispatch and all request-local native state.

#[path = "http_request.rs"]
mod http_request;

use std::borrow::Cow;
use std::collections::TryReserveError;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{self, Write as _};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use http_request::{HeadScanner, MAX_HEAD, RequestHead, parse_head};
use socket2::{SockRef, Socket};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

const IDLE: c_int = 0;
const READING: c_int = 1;
const RESPONDING: c_int = 2;
const RECEIVE_MS: c_int = 5000;
const SEND_MS: c_int = 1000;
const SEND_SLICE: usize = 64 * 1024;
const MAX_REPLY_HEAD: usize = 1024;

type Reply = unsafe extern "C" fn(
    *mut c_void,
    c_int,
    *const c_char,
    *const c_void,
    usize,
    c_int,
);
type Dispatch =
    unsafe extern "C" fn(*mut c_void, *const NativeRequest, Reply, *mut c_void);
type Finish =
    unsafe extern "C" fn(*mut c_void, *const NativeRequest, c_int, i64, usize);

#[repr(C)]
struct NativeRequest {
    method: [c_char; 16],
    path: [c_char; 2048],
    query: [c_char; 2048],
    http_minor: u8,
    origin: [c_char; 256],
    host: [c_char; 256],
    content_type: [c_char; 128],
    accept_language: [c_char; 256],
    body: *mut c_char,
    body_len: usize,
}

fn c_field<const N: usize>(source: &[u8]) -> [c_char; N] {
    let mut field = [0; N];
    for (target, &byte) in field.iter_mut().zip(source) {
        *target = byte as c_char;
    }
    field
}

impl NativeRequest {
    fn new(head: RequestHead<'_>, body: &mut [u8]) -> Self {
        Self {
            method: c_field(head.method),
            path: c_field(head.path),
            query: c_field(head.query),
            http_minor: head.http_minor,
            origin: c_field(head.origin),
            host: c_field(head.host),
            content_type: c_field(head.content_type),
            accept_language: c_field(head.accept_language),
            body: if body.is_empty() {
                ptr::null_mut()
            } else {
                body.as_mut_ptr().cast()
            },
            body_len: body.len().saturating_sub(1),
        }
    }
}

struct Transport {
    bound: Mutex<Option<(Runtime, TcpListener)>>,
    stop: watch::Sender<bool>,
    active_socket: Mutex<Option<Socket>>,
    port: u16,
    receive_ms: AtomicI32,
    send_buffer_bytes: AtomicI32,
    send_ms: AtomicI32,
    activity: AtomicI32,
}

impl Transport {
    fn bind(port: u16) -> io::Result<Self> {
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let listener = {
            let _enter = runtime.enter();
            let socket = TcpSocket::new_v4()?;
            #[cfg(unix)]
            socket.set_reuseaddr(true)?;
            #[cfg(windows)]
            set_exclusive_address_use(&socket)?;
            socket.bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())?;
            socket.listen(16)?
        };
        let port = listener.local_addr()?.port();
        Ok(Self {
            bound: Mutex::new(Some((runtime, listener))),
            stop: watch::channel(false).0,
            active_socket: Mutex::new(None),
            port,
            receive_ms: AtomicI32::new(RECEIVE_MS),
            send_buffer_bytes: AtomicI32::new(0),
            send_ms: AtomicI32::new(SEND_MS),
            activity: AtomicI32::new(IDLE),
        })
    }

    fn run(&self, context: *mut c_void, dispatch: Dispatch, finish: Finish) {
        let Some((runtime, listener)) =
            self.bound.lock().ok().and_then(|mut bound| bound.take())
        else {
            return;
        };
        let mut stop = self.stop.subscribe();
        runtime.block_on(async {
            while !*stop.borrow_and_update() {
                let accepted = tokio::select! {
                    biased;
                    _ = stop.changed() => break,
                    result = listener.accept() => result,
                };
                let Ok((mut stream, _)) = accepted else {
                    // Avoid a tight loop if the process has exhausted its file descriptors.
                    tokio::select! {
                        _ = stop.changed() => break,
                        _ = sleep(Duration::from_millis(200)) => continue,
                    }
                };
                let _ = stream.set_nodelay(true);
                // Keep a duplicate handle so stop can interrupt the peer even
                // if a synchronous native handler is blocking the run thread.
                let Ok(socket) = SockRef::from(&stream).try_clone() else {
                    continue;
                };
                let send_buffer = self.send_buffer_bytes.load(Ordering::Relaxed);
                if send_buffer > 0 {
                    let _ = socket.set_send_buffer_size(send_buffer as usize);
                }
                {
                    let mut active = self.active_socket.lock().unwrap();
                    if *stop.borrow() {
                        break;
                    }
                    *active = Some(socket);
                }
                let duration = millis(self.receive_ms.load(Ordering::Relaxed));
                let callbacks = Callbacks {
                    context,
                    dispatch,
                    finish,
                };
                serve_connection(
                    &mut stream,
                    duration,
                    millis(self.send_ms.load(Ordering::Relaxed)),
                    &mut stop,
                    &self.activity,
                    callbacks,
                )
                .await;
                self.active_socket.lock().unwrap().take();
                self.activity.store(IDLE, Ordering::Release);
            }
        });
        self.activity.store(IDLE, Ordering::Release);
    }

    fn stop(&self) {
        self.stop.send_replace(true);
        if let Some(socket) = self.active_socket.lock().unwrap().as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(windows)]
fn set_exclusive_address_use(socket: &TcpSocket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        SO_EXCLUSIVEADDRUSE, SOCKET_ERROR, SOL_SOCKET, WSAGetLastError,
        setsockopt,
    };

    // Winsock's SO_REUSEADDR permits rebinding an active listener. Require
    // the exclusive option before binding instead, as the C transport did.
    let enabled: c_int = 1;
    // SAFETY: the socket is live and enabled is an initialized Winsock BOOL.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            ptr::from_ref(&enabled).cast(),
            size_of::<c_int>() as c_int,
        )
    };
    if result == SOCKET_ERROR {
        // SAFETY: read the Winsock error on the thread that called setsockopt.
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
    }
    Ok(())
}

struct Callbacks {
    context: *mut c_void,
    dispatch: Dispatch,
    finish: Finish,
}

async fn serve_connection(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    duration: Duration,
    send_timeout: Duration,
    stop: &mut watch::Receiver<bool>,
    activity: &AtomicI32,
    callbacks: Callbacks,
) {
    activity.store(READING, Ordering::Release);
    let begin = Instant::now();
    let mut head_bytes = Vec::new();
    if head_bytes.try_reserve_exact(MAX_HEAD).is_err() {
        return;
    }
    head_bytes.resize(MAX_HEAD, 0);
    let received = tokio::select! {
        biased;
        _ = stop.changed() => return,
        result = timeout(duration, read_request(stream, &mut head_bytes)) => result,
    };
    let mut request = None;
    let mut response = None;
    let mut body;
    match received {
        Ok(Ok((head, bytes))) => {
            body = bytes;
            let native = NativeRequest::new(head, &mut body);
            if *stop.borrow() {
                return;
            }
            // SAFETY: the native host owns context until this run returns.
            // The callbacks and their arguments live through synchronous dispatch.
            unsafe {
                (callbacks.dispatch)(
                    callbacks.context,
                    &native,
                    capture_reply,
                    ptr::from_mut(&mut response).cast(),
                );
            }
            request = Some(native);
        }
        Ok(Err(ReadError::Status(status))) => {
            response = Some(Response::bad_request(status))
        }
        Ok(Err(ReadError::Closed)) => {}
        Err(_) => response = Some(Response::bad_request(408)),
    }
    let (status, bytes) = if let Some(response) = response {
        activity.store(RESPONDING, Ordering::Release);
        tokio::select! {
            biased;
            _ = stop.changed() => {}
            _ = timeout(response.write_timeout.unwrap_or(send_timeout), response.write(stream)) => {}
        }
        (response.status, response.body.len())
    } else {
        (0, 0)
    };
    let elapsed = begin.elapsed().as_millis().min(i64::MAX as u128) as i64;
    // SAFETY: context is live and request and body still belong to this iteration.
    // A null request with status 0 marks a connection closed before dispatch.
    unsafe {
        (callbacks.finish)(
            callbacks.context,
            request.as_ref().map_or(ptr::null(), ptr::from_ref),
            status,
            elapsed,
            bytes,
        );
    }
}

fn millis(ms: c_int) -> Duration {
    Duration::from_millis(ms as u64)
}

enum ReadError {
    Status(c_int),
    Closed,
}

async fn read_request<'a>(
    stream: &mut (impl AsyncRead + Unpin),
    bytes: &'a mut [u8],
) -> Result<(RequestHead<'a>, Vec<u8>), ReadError> {
    let mut have = 0;
    let mut scanner = HeadScanner::default();
    let head_end = loop {
        let count = stream
            .read(&mut bytes[have..])
            .await
            .map_err(|_| ReadError::Closed)?;
        if count == 0 {
            return Err(ReadError::Closed);
        }
        have += count;
        if let Some(end) = scanner
            .scan(&bytes[..have])
            .map_err(|error| ReadError::Status(error.status().into()))?
        {
            break end;
        }
    };
    let head = parse_head(&bytes[..head_end])
        .map_err(|error| ReadError::Status(error.status().into()))?;
    let length = head.content_length;
    let mut body = Vec::new();
    if length > 0 {
        body.try_reserve_exact(length + 1)
            .map_err(|_| ReadError::Closed)?;
        body.resize(length + 1, 0);
        let prefix = &bytes[head.body_offset..have];
        let copied = prefix.len().min(length);
        body[..copied].copy_from_slice(&prefix[..copied]);
        stream
            .read_exact(&mut body[copied..length])
            .await
            .map_err(|_| ReadError::Closed)?;
    }
    Ok((head, body))
}

struct Response {
    status: c_int,
    headers: Vec<u8>,
    body: Cow<'static, [u8]>,
    write_timeout: Option<Duration>,
}

impl Response {
    fn bad_request(status: c_int) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Cow::Borrowed(b"bad request"),
            write_timeout: None,
        }
    }

    fn allocation_failure() -> Self {
        Self {
            status: 500,
            headers: Vec::new(),
            body: Cow::Borrowed(b"internal server error"),
            write_timeout: None,
        }
    }

    async fn write(
        &self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            408 => "Request Timeout",
            411 => "Length Required",
            413 => "Content Too Large",
            415 => "Unsupported Media Type",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "",
        };
        // The fixed buffer also keeps response framing available after a
        // reply allocation fails.
        let mut head = [0; MAX_REPLY_HEAD];
        let mut cursor = io::Cursor::new(&mut head[..]);
        if (|| -> io::Result<()> {
            write!(cursor, "HTTP/1.1 {} {}\r\n", self.status, reason)?;
            io::Write::write_all(&mut cursor, &self.headers)?;
            write!(
                cursor,
                "Content-Length: {}\r\nConnection: close\r\n\r\n",
                self.body.len()
            )
        })()
        .is_err()
            || cursor.position() >= MAX_REPLY_HEAD as u64
        {
            return Ok(());
        }
        let head_len = cursor.position() as usize;
        stream.write_all(&head[..head_len]).await?;
        // Bound each write so Winsock cannot queue the full body into AFD
        // buffers before the send deadline or stop signal can be observed.
        for chunk in self.body.chunks(SEND_SLICE) {
            stream.write_all(chunk).await?;
        }
        Ok(())
    }
}

fn copy_reply_bytes(bytes: &[u8]) -> Result<Vec<u8>, TryReserveError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

unsafe extern "C" fn capture_reply(
    context: *mut c_void,
    status: c_int,
    headers: *const c_char,
    body: *const c_void,
    body_len: usize,
    send_deadline_ms: c_int,
) {
    // SAFETY: the transport passes an exclusive reference for this callback.
    // The host keeps the C string and non-empty body live until we return.
    let response = unsafe { &mut *context.cast::<Option<Response>>() };
    let headers = if headers.is_null() {
        &[]
    } else {
        unsafe { CStr::from_ptr(headers) }.to_bytes()
    };
    let body = if body_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(body.cast::<u8>(), body_len) }
    };
    let write_timeout = (1..=5000)
        .contains(&send_deadline_ms)
        .then(|| millis(send_deadline_ms));
    *response = Some(
        match copy_reply_bytes(headers).and_then(|headers| {
            copy_reply_bytes(body).map(|body| (headers, body))
        }) {
            Ok((headers, body)) => Response {
                status,
                headers,
                body: Cow::Owned(body),
                write_timeout,
            },
            Err(_) => Response::allocation_failure(),
        },
    );
}

#[unsafe(no_mangle)]
extern "C" fn mori_ui_http_bind(port: c_int) -> *mut Transport {
    u16::try_from(port)
        .ok()
        .and_then(|port| Transport::bind(port).ok())
        .map_or(ptr::null_mut(), |transport| {
            Box::into_raw(Box::new(transport))
        })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_port(transport: *const Transport) -> c_int {
    // SAFETY: the host passes the live owner returned by bind.
    unsafe { (*transport).port.into() }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_run(
    transport: *const Transport,
    context: *mut c_void,
    dispatch: Dispatch,
    finish: Finish,
) {
    // SAFETY: the host keeps the owner and callback context alive until run returns.
    unsafe { (*transport).run(context, dispatch, finish) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_stop(transport: *const Transport) {
    // SAFETY: stop may run concurrently, but the host keeps the owner live.
    unsafe { (*transport).stop() };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_free(transport: *mut Transport) -> bool {
    // SAFETY: the host frees the owner only after its run thread has stopped.
    drop(unsafe { Box::from_raw(transport) });
    true
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_activity(
    transport: *const Transport,
) -> c_int {
    // SAFETY: the host passes the live owner returned by bind.
    unsafe { (*transport).activity.load(Ordering::Acquire) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_set_receive_ms(
    transport: *const Transport,
    ms: c_int,
) {
    if ms > 0 {
        // SAFETY: the host passes the live owner returned by bind.
        unsafe { (*transport).receive_ms.store(ms, Ordering::Relaxed) };
    }
}

/// Test entry point for the native suite's request-framing matrix.
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_parse_head_for_test(
    data: *const u8,
    len: usize,
    request: *mut NativeRequest,
    body_offset: *mut usize,
    content_length: *mut usize,
) -> c_int {
    // SAFETY: the native test passes three live output slots and len readable
    // bytes. Permit an empty input without creating a slice from a null pointer.
    let data = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    unsafe {
        body_offset.write(0);
        content_length.write(0);
    }
    match HeadScanner::default()
        .scan(data)
        .and_then(|end| end.map(|end| parse_head(&data[..end])).transpose())
    {
        Ok(Some(head)) => {
            unsafe {
                body_offset.write(head.body_offset);
                content_length.write(head.content_length);
                request.write(NativeRequest::new(head, &mut []));
            }
            0
        }
        Ok(None) => -1,
        Err(error) => error.status().into(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_ui_http_set_send_options_for_test(
    transport: *const Transport,
    buffer_bytes: c_int,
    deadline_ms: c_int,
) {
    // SAFETY: the native test passes a live listener and configures it before run.
    let transport = unsafe { &*transport };
    transport
        .send_buffer_bytes
        .store(buffer_bytes, Ordering::Relaxed);
    if deadline_ms > 0 {
        transport.send_ms.store(deadline_ms, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream as Client;
    use std::thread;

    unsafe extern "C" fn echo(
        _: *mut c_void,
        request: *const NativeRequest,
        reply: Reply,
        reply_context: *mut c_void,
    ) {
        // SAFETY: the test's transport gives the callback a complete request.
        let request = unsafe { &*request };
        let raw_path = unsafe { CStr::from_ptr(request.path.as_ptr()) };
        let body_end = if request.body_len > 0 {
            unsafe { *request.body.add(request.body_len) }
        } else {
            0
        };
        let status = if raw_path.to_bytes() == b"/api%2Ftree" && body_end == 0 {
            202
        } else {
            400
        };
        unsafe {
            reply(
                reply_context,
                status,
                c"Content-Type: application/octet-stream\r\n".as_ptr(),
                request.body.cast(),
                request.body_len,
                0,
            );
        }
    }

    unsafe extern "C" fn discard_log(
        _: *mut c_void,
        _: *const NativeRequest,
        _: c_int,
        _: i64,
        _: usize,
    ) {
    }

    struct StopOnDrop<'a>(&'a Transport);

    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.stop();
        }
    }

    fn with_server(test: impl FnOnce(&Transport)) {
        let server = Transport::bind(0).unwrap();
        {
            let bound = server.bound.lock().unwrap();
            let (_, listener) = bound.as_ref().unwrap();
            assert_eq!(listener.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
        }
        thread::scope(|scope| {
            scope.spawn(|| server.run(ptr::null_mut(), echo, discard_log));
            let _stop = StopOnDrop(&server);
            test(&server);
        });
        Transport::bind(server.port).expect("shutdown released the port");
    }

    fn connect(server: &Transport) -> Client {
        let client =
            Client::connect((Ipv4Addr::LOCALHOST, server.port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client
    }

    fn exchange(input: &[u8], receive_deadline: Duration) -> Vec<u8> {
        Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let (mut client, mut host) = tokio::io::duplex(MAX_HEAD);
                let (_stop, mut stop) = watch::channel(false);
                let callbacks = Callbacks {
                    context: ptr::null_mut(),
                    dispatch: echo,
                    finish: discard_log,
                };
                let activity = AtomicI32::new(IDLE);
                let server = async {
                    serve_connection(
                        &mut host,
                        receive_deadline,
                        millis(SEND_MS),
                        &mut stop,
                        &activity,
                        callbacks,
                    )
                    .await;
                    drop(host);
                };
                let client = async {
                    client.write_all(input).await.unwrap();
                    let mut response = Vec::new();
                    client.read_to_end(&mut response).await.unwrap();
                    response
                };
                tokio::join!(server, client).1
            })
    }

    fn request(server: &Transport, bytes: &[u8]) -> Vec<u8> {
        let mut stream = connect(server);
        stream.write_all(bytes).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    }

    #[test]
    fn binds_loopback_only_and_preserves_raw_route_and_binary_body() {
        with_server(|server| {
            let response = request(
                server,
                b"POST /api%2Ftree HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\n\xff\0\n",
            );
            assert_eq!(
                response,
                b"HTTP/1.1 202 Accepted\r\nContent-Type: application/octet-stream\r\n\
                  Content-Length: 3\r\nConnection: close\r\n\r\n\xff\0\n",
            );
        });
    }

    #[test]
    fn copies_native_reply_and_closes_after_one_request() {
        let response = exchange(
            b"POST /api%2Ftree HTTP/1.1\r\nContent-Length: 3\r\n\r\n\xff\0\nGET / HTTP/1.1\r\n\r\n",
            Duration::from_secs(1),
        );
        assert_eq!(
            response,
            b"HTTP/1.1 202 Accepted\r\nContent-Type: application/octet-stream\r\n\
              Content-Length: 3\r\nConnection: close\r\n\r\n\xff\0\n",
        );
    }

    #[test]
    fn shutdown_interrupts_a_blocked_response() {
        Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let (mut client, mut host) = tokio::io::duplex(64);
                let (signal, mut stop) = watch::channel(false);
                let callbacks = Callbacks {
                    context: ptr::null_mut(),
                    dispatch: echo,
                    finish: discard_log,
                };
                let activity = AtomicI32::new(IDLE);
                let request = b"GET /api%2Ftree HTTP/1.0\r\n\r\n";
                client.write_all(request).await.unwrap();
                let server = serve_connection(
                    &mut host,
                    Duration::from_secs(1),
                    millis(SEND_MS),
                    &mut stop,
                    &activity,
                    callbacks,
                );
                let stop_while_writing = async {
                    assert_eq!(activity.load(Ordering::Acquire), RESPONDING);
                    signal.send_replace(true);
                };
                timeout(Duration::from_millis(500), async {
                    tokio::join!(biased; server, stop_while_writing);
                })
                .await
                .expect(
                    "stop interrupts a blocked write before its send deadline",
                );
            });
    }

    #[test]
    fn emits_transport_errors_before_dispatch() {
        for (input, status) in [
            (
                &b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"[..],
                &b"400"[..],
            ),
            (
                b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                b"411",
            ),
            (
                b"POST / HTTP/1.1\r\nContent-Length: 1048577\r\n\r\n",
                b"413",
            ),
            (&vec![b'a'; MAX_HEAD], b"431"),
        ] {
            let response = exchange(input, Duration::from_secs(1));
            assert_eq!(&response[9..12], status);
            assert!(response.ends_with(b"\r\n\r\nbad request"));
        }
    }

    #[test]
    fn missing_body_hits_receive_deadline() {
        let response = exchange(
            b"POST /api%2Ftree HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\r\na",
            Duration::from_millis(30),
        );
        assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout\r\n"));
    }

    #[test]
    fn shutdown_interrupts_a_partial_request_and_releases_the_port() {
        with_server(|server| {
            let mut socket = connect(server);
            socket.write_all(b"GET / HTTP/1.1\r\nHost: local").unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while server.activity.load(Ordering::Acquire) != READING {
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
            server.stop();
            match socket.read(&mut [0; 1]) {
                Ok(0) => {}
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {
                }
                result => panic!(
                    "expected shutdown to close the socket, got {result:?}"
                ),
            }
        });
    }
}
