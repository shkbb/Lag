//! The real HTTP transport: WinHTTP, like the updater (native TLS, system proxy, no C/C++ build dependency).
use crate::http::{Body, HttpClient, Request, Response};
use std::io::{self, Write};
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

struct Handle(*mut std::ffi::c_void);

// A WinHTTP session handle may be used from any thread; request handles never leave `send`.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    fn new(raw: *mut std::ffi::c_void) -> io::Result<Self> {
        if raw.is_null() {
            Err(io::Error::other(windows::core::Error::from_thread()))
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

pub struct WinHttp {
    session: Handle,
}

impl WinHttp {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let session = Handle::new(WinHttpOpen(
                w!("Lag-Cloud"),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ))?;
            // resolve, connect, send, receive: a stalled upload chunk fails after 2 min and is retried
            WinHttpSetTimeouts(session.0, 30_000, 30_000, 120_000, 120_000).map_err(io::Error::other)?;
            Ok(Self { session })
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

impl HttpClient for WinHttp {
    fn send(&self, request: Request, sink: &mut dyn Write) -> io::Result<Response> {
        let url: Vec<u16> = request.url.encode_utf16().collect();
        unsafe {
            let mut parts = URL_COMPONENTS {
                dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
                dwHostNameLength: u32::MAX,
                dwUrlPathLength: u32::MAX,
                dwExtraInfoLength: u32::MAX,
                ..Default::default()
            };
            WinHttpCrackUrl(&url, 0, &mut parts).map_err(io::Error::other)?;
            let mut host = std::slice::from_raw_parts(parts.lpszHostName.0, parts.dwHostNameLength as usize).to_vec();
            host.push(0);
            let mut path = Vec::new();
            if parts.dwUrlPathLength != 0 {
                path.extend_from_slice(std::slice::from_raw_parts(parts.lpszUrlPath.0, parts.dwUrlPathLength as usize));
            }
            if parts.dwExtraInfoLength != 0 {
                path.extend_from_slice(std::slice::from_raw_parts(parts.lpszExtraInfo.0, parts.dwExtraInfoLength as usize));
            }
            if path.is_empty() {
                path.push('/' as u16);
            }
            path.push(0);

            let connection = Handle::new(WinHttpConnect(self.session.0, PCWSTR(host.as_ptr()), parts.nPort, 0))?;
            let flags = if parts.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS {
                WINHTTP_FLAG_SECURE
            } else {
                WINHTTP_OPEN_REQUEST_FLAGS(0)
            };
            let method = wide(request.method.as_str());
            let handle = Handle::new(WinHttpOpenRequest(
                connection.0,
                PCWSTR(method.as_ptr()),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            ))?;
            if !request.follow_redirects {
                let off = WINHTTP_DISABLE_REDIRECTS.to_le_bytes();
                WinHttpSetOption(Some(handle.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&off)).map_err(io::Error::other)?;
            }
            let header_text: String = request.headers.iter().map(|(n, v)| format!("{n}: {v}\r\n")).collect();
            let header_wide: Vec<u16> = header_text.encode_utf16().collect();
            let headers = (!header_wide.is_empty()).then_some(header_wide.as_slice());

            match request.body {
                Body::Empty => WinHttpSendRequest(handle.0, headers, None, 0, 0, 0).map_err(io::Error::other)?,
                Body::Bytes(bytes) => {
                    let len = bytes.len() as u32;
                    WinHttpSendRequest(handle.0, headers, Some(bytes.as_ptr().cast()), len, len, 0)
                        .map_err(io::Error::other)?
                }
                Body::Stream { reader, len } => {
                    let total = u32::try_from(len).map_err(|_| io::Error::other("chunk over 4 GB"))?;
                    WinHttpSendRequest(handle.0, headers, None, 0, total, 0).map_err(io::Error::other)?;
                    let mut buf = vec![0u8; 256 * 1024];
                    let mut left = len;
                    while left > 0 {
                        let n = buf.len().min(left as usize);
                        reader.read_exact(&mut buf[..n])?;
                        let mut off = 0;
                        while off < n {
                            let mut written = 0u32;
                            WinHttpWriteData(handle.0, Some(buf[off..n].as_ptr().cast()), (n - off) as u32, &mut written)
                                .map_err(io::Error::other)?;
                            off += written as usize;
                        }
                        left -= n as u64;
                    }
                }
            }
            WinHttpReceiveResponse(handle.0, std::ptr::null_mut()).map_err(io::Error::other)?;

            let (mut status, mut size) = (0u32, 4u32);
            WinHttpQueryHeaders(
                handle.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(io::Error::other)?;

            let mut size = 0u32;
            let _ = WinHttpQueryHeaders(
                handle.0,
                WINHTTP_QUERY_RAW_HEADERS_CRLF,
                PCWSTR::null(),
                None,
                &mut size,
                std::ptr::null_mut(),
            );
            let mut raw = vec![0u16; size as usize / 2 + 1];
            let mut headers = Vec::new();
            if WinHttpQueryHeaders(
                handle.0,
                WINHTTP_QUERY_RAW_HEADERS_CRLF,
                PCWSTR::null(),
                Some(raw.as_mut_ptr().cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .is_ok()
            {
                let text = String::from_utf16_lossy(&raw[..size as usize / 2]);
                for line in text.lines().skip(1) {
                    if let Some((n, v)) = line.split_once(':') {
                        headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
                    }
                }
            }

            let ok = (200..300).contains(&status);
            let mut error_body = Vec::new();
            let mut buffer = vec![0u8; 256 * 1024];
            loop {
                let mut read = 0u32;
                WinHttpReadData(handle.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read)
                    .map_err(io::Error::other)?;
                if read == 0 {
                    break;
                }
                if ok {
                    sink.write_all(&buffer[..read as usize])?;
                } else if error_body.len() < 64 * 1024 {
                    error_body.extend_from_slice(&buffer[..read as usize]);
                }
            }
            Ok(Response { status: status as u16, headers, error_body })
        }
    }
}
