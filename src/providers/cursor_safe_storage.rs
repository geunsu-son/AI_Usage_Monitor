//! Decrypt Cursor desktop `cursor-safe-storage:v1:` tokens (Electron safeStorage).
//!
//! Cursor 3.27+ may store `cursorAuth/accessToken` as
//! `cursor-safe-storage:v1:` + base64(Electron safeStorage ciphertext).
//! On Windows that ciphertext is Chromium OSCrypt `v10` AES-256-GCM, keyed by
//! DPAPI-unprotected `os_crypt.encrypted_key` from `%APPDATA%\Cursor\Local State`.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use std::path::{Path, PathBuf};

const CURSOR_SAFE_STORAGE_PREFIX: &str = "cursor-safe-storage:v1:";
const DPAPI_PREFIX: &[u8] = b"DPAPI";
const V10_PREFIX: &[u8] = b"v10";
const GCM_NONCE_LEN: usize = 12;
const GCM_TAG_LEN: usize = 16;
const WINDOWS_KEY_LEN: usize = 32;

/// Resolve a stored Cursor access-token cell to a usable bearer JWT.
/// Plaintext JWTs are returned unchanged; encrypted envelopes are decrypted.
pub fn resolve_access_token(stored: &str) -> Result<String, ()> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    if let Some(payload) = trimmed.strip_prefix(CURSOR_SAFE_STORAGE_PREFIX) {
        return decrypt_cursor_safe_storage(payload);
    }
    // Legacy plaintext JWT (or any non-envelope value Cursor still accepts).
    Ok(trimmed.to_string())
}

fn decrypt_cursor_safe_storage(value_b64: &str) -> Result<String, ()> {
    let local_state = cursor_local_state_path().ok_or(())?;
    let key = windows_os_crypt_key(&local_state)?;
    let plaintext = decrypt_windows_v10(&key, value_b64)?;
    let token = String::from_utf8(plaintext).map_err(|_| ())?;
    if token.trim().is_empty() {
        return Err(());
    }
    Ok(token)
}

fn cursor_local_state_path() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(PathBuf::from(appdata).join("Cursor").join("Local State"))
}

fn windows_os_crypt_key(local_state: &Path) -> Result<[u8; WINDOWS_KEY_LEN], ()> {
    let raw = std::fs::read(local_state).map_err(|_| ())?;
    let root: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| ())?;
    let encoded = root
        .get("os_crypt")
        .and_then(|v| v.get("encrypted_key"))
        .and_then(|v| v.as_str())
        .ok_or(())?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| ())?;
    let dpapi_blob = decoded.strip_prefix(DPAPI_PREFIX).ok_or(())?;
    if dpapi_blob.is_empty() {
        return Err(());
    }
    let key = dpapi_unprotect(dpapi_blob)?;
    <[u8; WINDOWS_KEY_LEN]>::try_from(key.as_slice()).map_err(|_| ())
}

fn decrypt_windows_v10(key: &[u8; WINDOWS_KEY_LEN], value_b64: &str) -> Result<Vec<u8>, ()> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(value_b64.trim())
        .map_err(|_| ())?;
    let body = raw.strip_prefix(V10_PREFIX).ok_or(())?;
    if body.len() < GCM_NONCE_LEN + GCM_TAG_LEN {
        return Err(());
    }
    let (nonce_bytes, sealed) = body.split_at(GCM_NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    Aes256Gcm::new(key.into())
        .decrypt(nonce, sealed)
        .map_err(|_| ())
}

fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>, ()> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input_data = blob.to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input_data.len()).map_err(|_| ())?,
        pbData: input_data.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` points at `input_data` for the duration of the call;
    // on success `output.pbData` is LocalAlloc'd and freed below exactly once.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|_| ())?;
        if output.pbData.is_null() || output.cbData == 0 {
            return Err(());
        }
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData.cast()));
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

    #[test]
    fn plaintext_passthrough() {
        let jwt = "aaa.bbb.ccc";
        assert_eq!(resolve_access_token(jwt).unwrap(), jwt);
    }

    #[test]
    fn empty_rejected() {
        assert!(resolve_access_token("").is_err());
        assert!(resolve_access_token("   ").is_err());
    }

    #[test]
    fn decrypts_synthetic_windows_v10() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let plaintext = b"eyJhbGciOiJIUzI1NiJ9.e30.sig";
        let sealed = Aes256Gcm::new(&key.into())
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_slice())
            .unwrap();
        let mut raw = Vec::new();
        raw.extend_from_slice(V10_PREFIX);
        raw.extend_from_slice(&nonce);
        raw.extend_from_slice(&sealed);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        let out = decrypt_windows_v10(&key, &b64).unwrap();
        assert_eq!(out, plaintext);
    }
}
