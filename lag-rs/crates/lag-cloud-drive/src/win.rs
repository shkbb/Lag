//! What Windows itself gives the sign-in: SHA-256 and random bytes (PKCE), DPAPI encryption (the token store),
//! opening the user's default browser.
use std::io;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    assert!(status.is_ok(), "BCryptHash failed: {status:?}");
    out
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    let status = unsafe { BCryptGenRandom(None, &mut out, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    assert!(status.is_ok(), "BCryptGenRandom failed: {status:?}");
    out
}

/// Encrypts for the current Windows user (only they can decrypt it, on this PC).
pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    dpapi(data, true)
}

pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    dpapi(data, false)
}

fn dpapi(data: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if encrypt {
            CryptProtectData(&input, w!("Lag cloud sign-in"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        }
        .map_err(io::Error::other)?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(bytes)
    }
}

/// Free and total bytes of the drive or share holding `path` (UNC paths too).
pub fn disk_space(path: &std::path::Path) -> Option<lag_cloud::quota::ProviderSpace> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), Some(&mut total), None) }.ok()?;
    Some(lag_cloud::quota::ProviderSpace { free, total: Some(total) })
}

/// Opens `url` in the default browser.
pub fn open_browser(url: &str) -> io::Result<()> {
    let result =
        unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!("could not open the browser ({})", result.0 as isize)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_matches_the_standard() {
        let hex: String = sha256(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
    #[test]
    fn random_and_dpapi_round_trip() {
        assert_ne!(random_bytes::<32>(), random_bytes::<32>());
        let secret = b"refresh-token";
        let sealed = protect(secret).unwrap();
        assert_ne!(sealed.as_slice(), secret);
        assert_eq!(unprotect(&sealed).unwrap(), secret);
    }
}
