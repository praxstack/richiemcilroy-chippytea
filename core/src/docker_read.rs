//! Narrow, read-only Docker Engine accounting over a local Unix socket.
//!
//! The request method, API version, and route are fixed here. This client never
//! executes Docker, follows redirects, authenticates, or opens a TCP socket.

use crate::model::Result;
use serde::Deserialize;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{ErrorKind, Read, Write};
use std::mem::{offset_of, size_of};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const REQUEST: &[u8] = b"GET /v1.46/system/df?type=build-cache HTTP/1.1\r\n\
Host: docker\r\n\
Accept: application/json\r\n\
Connection: close\r\n\
\r\n";
const TIMEOUT: Duration = Duration::from_secs(15);
const POLL_SLICE: Duration = Duration::from_millis(20);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_ID_BYTES: usize = 512;
const MAX_KIND_BYTES: usize = 128;
const MAX_DESCRIPTION_BYTES: usize = 4096;

/// One numeric record returned by Docker Engine. Sizes remain logical daemon
/// accounting; none of these fields assert physical host recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildCacheRecord {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub size: u64,
    pub in_use: bool,
    pub shared: bool,
    pub usage_count: u64,
}

/// Bounded Docker Build Cache accounting. The two totals are checked sums of
/// owner-reported logical record sizes, never APFS or VM-disk recovery claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildCache {
    pub records: Vec<BuildCacheRecord>,
    pub logical_bytes: u64,
    pub unused_logical_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    uid: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SystemDfResponse {
    #[serde(deserialize_with = "deserialize_null_as_empty")]
    build_cache: Vec<RawBuildCacheRecord>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawBuildCacheRecord {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Type")]
    kind: String,
    description: String,
    in_use: bool,
    shared: bool,
    size: u64,
    usage_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyFraming {
    ContentLength(usize),
    Chunked,
}

fn deserialize_null_as_empty<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// Read only the Build Cache portion of Docker Engine's disk-usage response.
/// Cancellation closes the client connection promptly; it does not claim that
/// daemon-side accounting work has itself been cancelled.
pub(crate) fn build_cache(socket: &Path, cancel: &AtomicBool) -> Result<BuildCache> {
    build_cache_with_timeout(socket, cancel, TIMEOUT)
}

fn build_cache_with_timeout(
    socket: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<BuildCache> {
    if timeout.is_zero() || timeout > TIMEOUT {
        return Err("Docker read deadline is invalid".into());
    }
    let deadline = Instant::now() + timeout;
    check_cancel(cancel)?;
    let expected_identity = socket_identity(socket)?;
    let mut stream = connect_nonblocking(socket, cancel, deadline)?;
    write_all_nonblocking(&mut stream, REQUEST, cancel, deadline)?;
    let response = read_response(&mut stream, cancel, deadline)?;
    let observed_identity = socket_identity(socket)?;
    check_cancel(cancel)?;
    check_deadline(deadline)?;
    if observed_identity != expected_identity {
        return Err("Docker Unix socket identity changed during the read".into());
    }
    parse_response(&response)
}

fn socket_identity(path: &Path) -> Result<SocketIdentity> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("Docker Unix socket path is not absolute and normalized".into());
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Docker Unix socket metadata is unavailable: {error}"))?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Docker endpoint is not an owned Unix socket".into());
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        uid: metadata.uid(),
    })
}

fn connect_nonblocking(path: &Path, cancel: &AtomicBool, deadline: Instant) -> Result<UnixStream> {
    let path_bytes = os_bytes(path.as_os_str())?;
    let path_capacity = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
        .sun_path
        .len();
    if path_bytes.is_empty() || path_bytes.len() >= path_capacity {
        return Err("Docker Unix socket path is empty or too long".into());
    }

    let raw_fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw_fd < 0 {
        return Err(format!(
            "Docker Unix socket could not be created: {}",
            std::io::Error::last_os_error()
        ));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    set_fd_flags(fd.as_raw_fd())?;

    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(path_bytes.iter().copied()) {
        *target = source as libc::c_char;
    }
    let address_len = offset_of!(libc::sockaddr_un, sun_path)
        .checked_add(path_bytes.len())
        .and_then(|length| length.checked_add(1))
        .and_then(|length| libc::socklen_t::try_from(length).ok())
        .ok_or("Docker Unix socket address length overflowed")?;
    #[cfg(target_vendor = "apple")]
    {
        address.sun_len = u8::try_from(address_len)
            .map_err(|_| "Docker Unix socket address length is unsupported")?;
    }

    check_cancel(cancel)?;
    let connected = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const address).cast::<libc::sockaddr>(),
            address_len,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(code)
                if code == libc::EINPROGRESS
                    || code == libc::EALREADY
                    || code == libc::EAGAIN
        ) {
            return Err(format!("Docker Unix socket connection failed: {error}"));
        }
        wait_ready(fd.as_raw_fd(), libc::POLLOUT, cancel, deadline)?;
        let socket_error = socket_error(fd.as_raw_fd())?;
        if socket_error != 0 {
            return Err(format!(
                "Docker Unix socket connection failed: {}",
                std::io::Error::from_raw_os_error(socket_error)
            ));
        }
    }
    check_cancel(cancel)?;
    Ok(UnixStream::from(fd))
}

fn os_bytes(value: &OsStr) -> Result<&[u8]> {
    let bytes = value.as_bytes();
    if bytes.contains(&0) {
        return Err("Docker Unix socket path contains a NUL byte".into());
    }
    Ok(bytes)
}

fn set_fd_flags(fd: libc::c_int) -> Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        // This also runs in a Swift-hosted static library, where Rust's binary
        // startup cannot be relied on to ignore SIGPIPE. A peer disappearing
        // must return an I/O error, never terminate the native application.
        let enabled: libc::c_int = 1;
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&raw const enabled).cast(),
                size_of::<libc::c_int>() as libc::socklen_t,
            )
        } != 0
        {
            return Err("Docker Unix socket could not disable SIGPIPE".into());
        }
    }
    let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if descriptor_flags < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } < 0
    {
        return Err(format!(
            "Docker Unix socket close-on-exec could not be set: {}",
            std::io::Error::last_os_error()
        ));
    }
    let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if status_flags < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFL, status_flags | libc::O_NONBLOCK) } < 0
    {
        return Err(format!(
            "Docker Unix socket nonblocking mode could not be set: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn socket_error(fd: libc::c_int) -> Result<libc::c_int> {
    let mut error = 0;
    let mut length = libc::socklen_t::try_from(size_of::<libc::c_int>())
        .map_err(|_| "Docker Unix socket error length is unsupported")?;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&raw mut error).cast(),
            &raw mut length,
        )
    } != 0
    {
        return Err(format!(
            "Docker Unix socket connection status is unavailable: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(error)
}

fn write_all_nonblocking(
    stream: &mut UnixStream,
    bytes: &[u8],
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        check_cancel(cancel)?;
        check_deadline(deadline)?;
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err("Docker Unix socket closed while sending the request".into()),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                wait_ready(stream.as_raw_fd(), libc::POLLOUT, cancel, deadline)?;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("Docker request could not be sent: {error}")),
        }
    }
    Ok(())
}

fn read_response(
    stream: &mut UnixStream,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>> {
    let mut response = Vec::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    loop {
        check_cancel(cancel)?;
        check_deadline(deadline)?;
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(response),
            Ok(count) => {
                if count > MAX_RESPONSE_BYTES.saturating_sub(response.len()) {
                    return Err("Docker response exceeded the 64 KiB limit".into());
                }
                response.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                wait_ready(stream.as_raw_fd(), libc::POLLIN, cancel, deadline)?;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("Docker response could not be read: {error}")),
        }
    }
}

fn wait_ready(
    fd: libc::c_int,
    events: libc::c_short,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<()> {
    loop {
        check_cancel(cancel)?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Docker read timed out")?;
        let wait = remaining.min(POLL_SLICE);
        let timeout_ms = wait.as_millis().max(1).min(i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
        if ready > 0 {
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err("Docker Unix socket became invalid".into());
            }
            if descriptor.revents & (events | libc::POLLERR | libc::POLLHUP) != 0 {
                return Ok(());
            }
        } else if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                return Err(format!("Docker Unix socket polling failed: {error}"));
            }
        }
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err("Docker read cancelled; daemon-side accounting may still finish".into())
    } else {
        Ok(())
    }
}

fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        Err("Docker read timed out; daemon-side accounting may still finish".into())
    } else {
        Ok(())
    }
}

fn parse_response(response: &[u8]) -> Result<BuildCache> {
    let header_end = find_bytes(response, b"\r\n\r\n")
        .ok_or("Docker response ended before its HTTP headers were complete")?;
    let headers = &response[..header_end];
    let body = &response[header_end + 4..];
    let (status, framing, content_type) = parse_headers(headers)?;
    if status != 200 {
        return Err(format!("Docker Engine returned HTTP status {status}"));
    }
    if content_type
        .split(';')
        .next()
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err("Docker Engine response was not application/json".into());
    }
    let decoded = match framing {
        BodyFraming::ContentLength(length) => {
            if length != body.len() {
                return Err("Docker response Content-Length did not match its body".into());
            }
            body.to_vec()
        }
        BodyFraming::Chunked => decode_chunked(body)?,
    };
    parse_body(&decoded)
}

fn parse_headers(headers: &[u8]) -> Result<(u16, BodyFraming, String)> {
    let text =
        std::str::from_utf8(headers).map_err(|_| "Docker response headers were not valid UTF-8")?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().ok_or("Docker response had no status line")?;
    let mut status_parts = status_line.splitn(3, ' ');
    if status_parts.next() != Some("HTTP/1.1") {
        return Err("Docker response was not HTTP/1.1".into());
    }
    let status_text = status_parts
        .next()
        .filter(|value| value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("Docker response had a malformed status")?;
    let status = status_text
        .parse::<u16>()
        .map_err(|_| "Docker response status overflowed")?;

    let mut content_length = None;
    let mut transfer_encoding = None;
    let mut content_type = None;
    for line in lines {
        if line.is_empty() || line.starts_with(' ') || line.starts_with('\t') {
            return Err("Docker response contained a malformed header".into());
        }
        let (name, value) = line
            .split_once(':')
            .ok_or("Docker response contained a header without a colon")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err("Docker response contained an invalid header name".into());
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some()
                || value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err("Docker response had an invalid Content-Length".into());
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| "Docker response Content-Length overflowed")?;
            if length > MAX_RESPONSE_BYTES {
                return Err("Docker response body exceeded the 64 KiB limit".into());
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if transfer_encoding.replace(value.to_owned()).is_some() {
                return Err("Docker response repeated Transfer-Encoding".into());
            }
        } else if name.eq_ignore_ascii_case("content-type")
            && content_type.replace(value.to_owned()).is_some()
        {
            return Err("Docker response repeated Content-Type".into());
        } else if name.eq_ignore_ascii_case("content-encoding")
            && !value.eq_ignore_ascii_case("identity")
        {
            return Err("Docker response used an unsupported content encoding".into());
        }
    }
    let content_type = content_type.ok_or("Docker response omitted Content-Type")?;
    let framing = match (content_length, transfer_encoding) {
        (Some(_), Some(_)) => return Err("Docker response used ambiguous body framing".into()),
        (Some(length), None) => BodyFraming::ContentLength(length),
        (None, Some(encoding)) if encoding.eq_ignore_ascii_case("chunked") => BodyFraming::Chunked,
        (None, Some(_)) => {
            return Err("Docker response used an unsupported Transfer-Encoding".into());
        }
        (None, None) => return Err("Docker response omitted explicit body framing".into()),
    };
    Ok((status, framing, content_type))
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>> {
    let mut offset = 0;
    let mut decoded = Vec::new();
    loop {
        let line_end = find_bytes(&body[offset..], b"\r\n")
            .map(|relative| offset + relative)
            .ok_or("Docker chunked response ended before a chunk size")?;
        let size_text = std::str::from_utf8(&body[offset..line_end])
            .map_err(|_| "Docker chunk size was not valid UTF-8")?;
        if size_text.is_empty()
            || size_text.len() > 16
            || !size_text.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Docker chunk size was malformed".into());
        }
        let size =
            usize::from_str_radix(size_text, 16).map_err(|_| "Docker chunk size overflowed")?;
        offset = line_end + 2;
        if size == 0 {
            if body.get(offset..) != Some(b"\r\n") {
                return Err("Docker chunked response had trailers or trailing bytes".into());
            }
            return Ok(decoded);
        }
        let end = offset
            .checked_add(size)
            .ok_or("Docker chunk length overflowed")?;
        if end.checked_add(2).is_none_or(|value| value > body.len())
            || body.get(end..end + 2) != Some(b"\r\n")
        {
            return Err("Docker chunked response ended inside a chunk".into());
        }
        if size > MAX_RESPONSE_BYTES.saturating_sub(decoded.len()) {
            return Err("Docker decoded body exceeded the 64 KiB limit".into());
        }
        decoded.extend_from_slice(&body[offset..end]);
        offset = end + 2;
    }
}

fn parse_body(body: &[u8]) -> Result<BuildCache> {
    let response: SystemDfResponse = serde_json::from_slice(body)
        .map_err(|error| format!("Docker Build Cache JSON was malformed: {error}"))?;
    if response.build_cache.len() > MAX_RECORDS {
        return Err("Docker Build Cache response exceeded 256 records".into());
    }
    let mut ids = HashSet::with_capacity(response.build_cache.len());
    let mut records = Vec::with_capacity(response.build_cache.len());
    let mut logical_bytes = 0u64;
    let mut unused_logical_bytes = 0u64;
    for record in response.build_cache {
        validate_record_text("ID", &record.id, MAX_ID_BYTES, false)?;
        validate_record_text("type", &record.kind, MAX_KIND_BYTES, false)?;
        validate_record_text(
            "description",
            &record.description,
            MAX_DESCRIPTION_BYTES,
            true,
        )?;
        if !ids.insert(record.id.clone()) {
            return Err("Docker Build Cache response contained a duplicate ID".into());
        }
        logical_bytes = logical_bytes
            .checked_add(record.size)
            .ok_or("Docker Build Cache logical bytes overflowed")?;
        if !record.in_use {
            unused_logical_bytes = unused_logical_bytes
                .checked_add(record.size)
                .ok_or("Docker Build Cache unused logical bytes overflowed")?;
        }
        records.push(BuildCacheRecord {
            id: record.id,
            kind: record.kind,
            description: record.description,
            size: record.size,
            in_use: record.in_use,
            shared: record.shared,
            usage_count: record.usage_count,
        });
    }
    Ok(BuildCache {
        records,
        logical_bytes,
        unused_logical_bytes,
    })
}

fn validate_record_text(
    name: &str,
    value: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<()> {
    if (!allow_empty && value.is_empty())
        || value.len() > max_bytes
        || value.as_bytes().contains(&0)
        || (!allow_empty && value.chars().any(char::is_control))
    {
        return Err(format!("Docker Build Cache {name} was invalid"));
    }
    Ok(())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use tempfile::TempDir;

    #[cfg(target_vendor = "apple")]
    #[test]
    fn raw_sockets_disable_sigpipe_for_swift_hosted_library_calls() {
        let mut descriptors = [-1; 2];
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_STREAM,
                    0,
                    descriptors.as_mut_ptr(),
                )
            },
            0
        );
        let mut stream = UnixStream::from(unsafe { OwnedFd::from_raw_fd(descriptors[0]) });
        let peer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        set_fd_flags(stream.as_raw_fd()).unwrap();
        let mut enabled: libc::c_int = 0;
        let mut length = size_of::<libc::c_int>() as libc::socklen_t;
        assert_eq!(
            unsafe {
                libc::getsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_NOSIGPIPE,
                    (&raw mut enabled).cast(),
                    &raw mut length,
                )
            },
            0
        );
        assert_eq!(enabled, 1);
        drop(peer);
        assert!(stream.write(b"fixture").is_err());
    }

    struct Server {
        _directory: TempDir,
        path: std::path::PathBuf,
        request: Arc<Mutex<Vec<u8>>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Server {
        fn response(response: Vec<u8>) -> Self {
            Self::serve(move |mut stream| {
                stream.write_all(&response).unwrap();
            })
        }

        fn serve(handler: impl FnOnce(UnixStream) + Send + 'static) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("docker.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let request = Arc::new(Mutex::new(Vec::new()));
            let request_for_thread = Arc::clone(&request);
            let thread = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0u8; 1024];
                loop {
                    let count = stream.read(&mut bytes).unwrap();
                    if count == 0 {
                        return;
                    }
                    let mut request = request_for_thread.lock().unwrap();
                    request.extend_from_slice(&bytes[..count]);
                    if find_bytes(&request, b"\r\n\r\n").is_some() {
                        drop(request);
                        handler(stream);
                        return;
                    }
                    assert!(request.len() <= 4096);
                }
            });
            Self {
                _directory: directory,
                path,
                request,
                thread: Some(thread),
            }
        }

        fn join(mut self) -> Vec<u8> {
            self.thread.take().unwrap().join().unwrap();
            self.request.lock().unwrap().clone()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn json_response(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .into_bytes()
    }

    fn record(id: &str, size: u64, in_use: bool, shared: bool) -> String {
        format!(
            "{{\"ID\":{id:?},\"Type\":\"regular\",\"Description\":\"fixture\",\"InUse\":{in_use},\"Shared\":{shared},\"Size\":{size},\"UsageCount\":1}}"
        )
    }

    #[test]
    fn sends_only_fixed_build_cache_get_and_keeps_numeric_logical_accounting() {
        let body = format!(
            "{{\"BuildCache\":[{},{}]}}",
            record("unused", 51, false, true),
            record("active", 7, true, false)
        );
        let server = Server::response(json_response(&body));
        let result = build_cache(&server.path, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.logical_bytes, 58);
        assert_eq!(result.unused_logical_bytes, 51);
        assert_eq!(result.records.len(), 2);
        assert!(result.records[0].shared);
        let request = server.join();
        assert_eq!(request, REQUEST);
        assert!(!request.windows(5).any(|window| window == b"POST "));
        assert!(!request.windows(7).any(|window| window == b"DELETE "));
    }

    #[test]
    fn accepts_strict_chunked_json() {
        let body = format!("{{\"BuildCache\":[{}]}}", record("one", 3, false, false));
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        )
        .into_bytes();
        let server = Server::response(response);
        assert_eq!(
            build_cache(&server.path, &AtomicBool::new(false))
                .unwrap()
                .unused_logical_bytes,
            3
        );
    }

    #[test]
    fn accepts_null_as_empty_but_requires_the_build_cache_field_and_real_bools() {
        let server = Server::response(json_response("{\"BuildCache\":null}"));
        assert!(
            build_cache(&server.path, &AtomicBool::new(false))
                .unwrap()
                .records
                .is_empty()
        );

        for body in [
            "{}".to_owned(),
            "{\"BuildCache\":[{\"ID\":\"one\",\"Type\":\"regular\",\"Description\":\"fixture\",\"InUse\":0,\"Shared\":false,\"Size\":1,\"UsageCount\":1}]}".to_owned(),
            "{\"BuildCache\":[{\"ID\":\"one\",\"Type\":\"regular\",\"Description\":\"fixture\",\"InUse\":false,\"Shared\":\"false\",\"Size\":1,\"UsageCount\":1}]}".to_owned(),
        ] {
            let server = Server::response(json_response(&body));
            assert!(build_cache(&server.path, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn rejects_redirects_http_errors_and_ambiguous_framing() {
        for response in [
            b"HTTP/1.1 302 Found\r\nContent-Type: application/json\r\nContent-Length: 0\r\nLocation: http://remote/\r\n\r\n".to_vec(),
            b"HTTP/1.1 500 Error\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n{}".to_vec(),
        ] {
            let server = Server::response(response);
            assert!(build_cache(&server.path, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn rejects_malformed_oversized_and_premature_eof_responses() {
        let responses = [
            json_response("{bad}"),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 65537\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{}".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}".to_vec(),
        ];
        for response in responses {
            let server = Server::response(response);
            assert!(build_cache(&server.path, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn rejects_too_many_records_duplicate_ids_and_sum_overflow() {
        let too_many = (0..=MAX_RECORDS)
            .map(|index| record(&format!("id-{index}"), 1, false, false))
            .collect::<Vec<_>>()
            .join(",");
        let bodies = [
            format!("{{\"BuildCache\":[{too_many}]}}"),
            format!(
                "{{\"BuildCache\":[{},{}]}}",
                record("same", 1, false, false),
                record("same", 2, false, false)
            ),
            format!(
                "{{\"BuildCache\":[{},{}]}}",
                record("first", u64::MAX, false, false),
                record("second", 1, false, false)
            ),
        ];
        for body in bodies {
            assert!(body.len() < MAX_RESPONSE_BYTES);
            let server = Server::response(json_response(&body));
            assert!(build_cache(&server.path, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn stalled_response_times_out_without_waiting_for_server_completion() {
        // Prepare both endpoints before starting the read deadline.
        let (mut stream, peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
            drop(peer);
        });
        let result = read_response(
            &mut stream,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_millis(40),
        );
        let released = release_tx.send(());
        server.join().unwrap();

        released.expect("the fake daemon stopped before the read timed out");
        let error = result.unwrap_err();
        assert!(error.contains("timed out"));
    }

    #[test]
    fn cancellation_returns_promptly_and_does_not_claim_daemon_cancellation() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = Server::serve(move |mut stream| {
            ready_tx.send(()).unwrap();
            // Keep the daemon connection open until the client has returned.
            // This bound is a cleanup watchdog, not a performance assertion.
            release_rx
                .recv_timeout(TIMEOUT)
                .expect("stalled server was not released");
            stream.set_nonblocking(true).unwrap();
            assert_eq!(
                stream.read(&mut [0u8; 1]).unwrap(),
                0,
                "the client must close its connection without a daemon response"
            );
        });
        let path = server.path.clone();
        let client_cancel = Arc::clone(&cancel);
        let (result_tx, result_rx) = mpsc::channel();
        let client = thread::spawn(move || {
            let result = build_cache(&path, &client_cancel);
            let _ = result_tx.send(result);
        });

        let ready = ready_rx.recv_timeout(Duration::from_secs(5));
        if ready.is_ok() {
            cancel.store(true, Ordering::Release);
        }
        // Start the watchdog after the request handshake, excluding startup.
        // It remains well below the normal 15-second request deadline.
        let result = result_rx.recv_timeout(Duration::from_secs(2));
        let released = release_tx.send(());
        // Wake accept if the client failed before connecting, so cleanup cannot hang.
        drop(UnixStream::connect(&server.path));
        let client_joined = client.join();
        let request = server.join();

        // Release and join both threads before any assertion can unwind this test.
        client_joined.unwrap();
        ready.expect("the fake daemon did not receive the request");
        released.expect("the fake daemon stopped before the client returned");
        assert_eq!(request, REQUEST);
        let error = result
            .expect("the client did not return while the fake daemon was stalled")
            .unwrap_err();
        assert_eq!(
            error,
            "Docker read cancelled; daemon-side accounting may still finish"
        );
    }
}
