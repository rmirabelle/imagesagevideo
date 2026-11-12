use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};
use std::path::Path;

/**
 * Encryption for the files this app keeps for itself: its settings and its
 * job list. Each file is sealed with XChaCha20-Poly1305. The key is made on
 * first use and stays in Windows Credential Manager, so only this Windows
 * account can read the files. Finished videos are not sealed: they are
 * normal WebM files that any player opens.
 *
 * A sealed file is the magic bytes, a 24-byte nonce, then the ciphertext.
 */
const MAGIC: &[u8; 4] = b"ISV1";
const NONCE_BYTES: usize = 24;
const KEY_SERVICE: &str = "ImageSageVideo";
const KEY_NAME: &str = "file-key";

fn key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEY_SERVICE, KEY_NAME).map_err(|error| format!("Could not open Windows Credential Manager: {error}"))
}

/// The file key from Windows Credential Manager; with `create`, a new key is made when there is none.
fn file_key(create: bool) -> Result<Option<Vec<u8>>, String> {
    let entry = key_entry()?;
    match entry.get_password() {
        Ok(encoded) => {
            let key = STANDARD.decode(encoded.trim()).map_err(|_| "The saved file key is not valid.".to_string())?;
            if key.len() != 32 {
                return Err("The saved file key is not valid.".into());
            }
            Ok(Some(key))
        }
        Err(keyring::Error::NoEntry) if create => {
            let key = XChaCha20Poly1305::generate_key(&mut OsRng).to_vec();
            entry
                .set_password(&STANDARD.encode(&key))
                .map_err(|error| format!("Could not save the file key in Windows Credential Manager: {error}"))?;
            Ok(Some(key))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("Could not read the file key from Windows Credential Manager: {error}")),
    }
}

fn seal(key: &[u8], plain: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| "The file key is not valid.".to_string())?;
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let sealed = cipher.encrypt(&nonce, plain).map_err(|_| "Could not encrypt the file.".to_string())?;
    let mut bytes = Vec::with_capacity(MAGIC.len() + NONCE_BYTES + sealed.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&sealed);
    Ok(bytes)
}

fn open(key: &[u8], bytes: &[u8]) -> Result<Vec<u8>, String> {
    let damaged = || "The file is damaged or was made on another Windows account.".to_string();
    if bytes.len() < MAGIC.len() + NONCE_BYTES || &bytes[..MAGIC.len()] != MAGIC {
        return Err(damaged());
    }
    let (nonce, sealed) = bytes[MAGIC.len()..].split_at(NONCE_BYTES);
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| "The file key is not valid.".to_string())?;
    cipher.decrypt(XNonce::from_slice(nonce), sealed).map_err(|_| damaged())
}

/// Encrypts `plain` and writes it to `path`. It writes beside the file first, so a failed write keeps the old file.
pub fn write_sealed(path: &Path, plain: &[u8]) -> Result<(), String> {
    let key = file_key(true)?.ok_or("There is no file key.")?;
    let bytes = seal(&key, plain)?;
    let temporary = path.with_extension("saving");
    std::fs::write(&temporary, bytes).map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("Could not replace {}: {error}", path.display())
    })
}

/// Reads and decrypts `path`. None when the file does not exist or there is no key yet.
pub fn read_sealed(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    let Some(key) = file_key(false)? else { return Ok(None) };
    open(&key, &bytes).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_file_opens_with_its_key_only() {
        let key = [7_u8; 32];
        let sealed = seal(&key, b"model paths").unwrap();
        assert!(!sealed.windows(11).any(|window| window == b"model paths"));
        assert_eq!(open(&key, &sealed).unwrap(), b"model paths");
        assert!(open(&[8_u8; 32], &sealed).is_err());
    }

    #[test]
    fn a_changed_file_is_refused() {
        let key = [7_u8; 32];
        let mut sealed = seal(&key, b"job list").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(open(&key, &sealed).is_err());
        assert!(open(&key, b"ISV1").is_err());
        assert!(open(&key, b"plain text that is long enough to hold a nonce").is_err());
    }
}
