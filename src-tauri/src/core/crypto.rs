use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{Context, Result};
use std::path::Path;

pub const ENC_PREFIX: &str = "enc:v1:";

/// Load an existing 32-byte key from `path`, or generate and persist a new one.
///
/// If the file exists but has an unexpected size, returns an error instead of
/// silently regenerating — regenerating would make all previously encrypted
/// values permanently undecryptable.
pub fn load_or_create_key(path: &Path) -> Result<[u8; 32]> {
    if path.exists() {
        let bytes = std::fs::read(path).context("Failed to read secret key file")?;
        anyhow::ensure!(
            bytes.len() == 32,
            "Secret key file '{}' is corrupted ({} bytes, expected 32). \
             Delete it manually to regenerate, but note that existing encrypted values will be lost.",
            path.display(),
            bytes.len()
        );
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        // Re-assert the restriction on load, not just at creation. Every key
        // written before this was enforced keeps its original permissions
        // otherwise — on the machine this was written for, the live key was
        // readable by a local group months after creation. Cheap: the only
        // caller is `SkillStore::new`, once per process.
        set_owner_readonly(path);
        return Ok(key);
    }

    let mut key = [0u8; 32];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut key);
    // Build the file at a temporary path — created empty, restricted, then
    // filled — and move it into place only once it is both complete and
    // private. Writing to the real path and restricting afterwards would leave
    // the key readable for the length of two syscalls; doing the empty-then-fill
    // dance on the real path instead would leave a zero-byte file behind after
    // an ill-timed crash, and `load_or_create_key` refuses to regenerate over a
    // wrong-sized key on purpose, so that would be a manual-repair trap.
    let staging = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&staging, []).context("Failed to create secret key file")?;
    set_owner_readonly(&staging);
    std::fs::write(&staging, key).context("Failed to write secret key file")?;
    // Same directory, so the rename is atomic and carries the restriction with
    // it rather than re-inheriting the parent's.
    std::fs::rename(&staging, path).context("Failed to install secret key file")?;
    Ok(key)
}

/// On Unix, restrict the key file to owner-only access (0600).
#[cfg(unix)]
fn set_owner_readonly(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

/// On Windows, the closest equivalent of 0600: drop the inherited ACL and
/// re-grant only the current user and SYSTEM (SYSTEM being the analogue of
/// root, which reads 0600 files anyway).
///
/// Not skippable by arguing the profile directory is already private. On the
/// machine this was written for, `~/.patchbay` carried a non-inherited ACE
/// granting a local group read, and a fresh file under the profile tree
/// inherited six ACEs including two unresolved SIDs with Modify. Inheritance
/// is whatever the box happens to be configured with; the key file should not
/// depend on it.
///
/// Best-effort, like the unix branch: a key that could not be locked down is
/// still better than no key at all, and the caller has no recovery path.
#[cfg(windows)]
fn set_owner_readonly(path: &Path) {
    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .arg("*S-1-5-18:F")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn set_owner_readonly(_path: &Path) {}

/// Encrypt `plaintext` with AES-256-GCM and return an `enc:v1:<hex>` string.
pub fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let mut nonce_bytes = [0u8; 12];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| anyhow::anyhow!("Encryption failed"))?;

    // Store as hex(nonce || ciphertext) so the result is printable ASCII.
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(format!("{}{}", ENC_PREFIX, hex::encode(&combined)))
}

/// Decrypt an `enc:v1:<hex>` string.  Returns an error if the value is not
/// a recognised encrypted blob (callers can detect plaintext via `is_encrypted`).
pub fn decrypt(key: &[u8; 32], value: &str) -> Result<String> {
    let hex_str = value
        .strip_prefix(ENC_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("Value is not an encrypted blob"))?;

    let combined = hex::decode(hex_str).context("Invalid hex in encrypted value")?;
    if combined.len() < 12 {
        anyhow::bail!("Encrypted value is too short to contain a nonce");
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("Decryption failed — key mismatch or data corrupted"))?;

    String::from_utf8(plaintext).context("Decrypted bytes are not valid UTF-8")
}

/// Returns true if `value` looks like it was produced by `encrypt()`.
pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(ENC_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [0x42u8; 32]
    }

    #[test]
    fn round_trip() {
        let key = test_key();
        let plaintext = "https://ghp_secret@github.com/user/repo.git";
        let encrypted = encrypt(&key, plaintext).unwrap();
        assert!(is_encrypted(&encrypted));
        assert_ne!(encrypted, plaintext);
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn different_nonces_each_time() {
        let key = test_key();
        let a = encrypt(&key, "same").unwrap();
        let b = encrypt(&key, "same").unwrap();
        // Nonces are random, so ciphertexts must differ even for the same plaintext.
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_fails() {
        let key_a = test_key();
        let key_b = [0x99u8; 32];
        let encrypted = encrypt(&key_a, "secret").unwrap();
        assert!(decrypt(&key_b, &encrypted).is_err());
    }

    #[test]
    fn is_encrypted_detects_prefix() {
        assert!(is_encrypted("enc:v1:deadbeef"));
        assert!(!is_encrypted("https://user:pass@host"));
        assert!(!is_encrypted(""));
    }

    #[test]
    fn load_or_create_key_creates_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".secret.key");

        let key1 = load_or_create_key(&path).unwrap();
        assert!(path.exists());

        let key2 = load_or_create_key(&path).unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn corrupted_key_file_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".secret.key");
        // Write a file with wrong length.
        std::fs::write(&path, b"too-short").unwrap();
        let err = load_or_create_key(&path).unwrap_err();
        assert!(err.to_string().contains("corrupted"));
    }

    #[cfg(unix)]
    #[test]
    fn key_file_has_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".secret.key");
        load_or_create_key(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// Windows counterpart of the 0600 test. A tempdir sits under the user
    /// profile, so the key file starts out with inherited ACEs — `(I)` in
    /// icacls output. After creation there must be none left, which is what
    /// stops a stray group ACE on the profile tree from reaching the key.
    /// A key written before the restriction was enforced must be tightened the
    /// next time it is loaded, not left as-is forever.
    #[test]
    fn loading_an_existing_key_re_restricts_it() {
        // The Windows half needs a fixture that *starts* with inherited ACEs,
        // so it must sit under the user profile. Do not fall back to the
        // default temp directory: on a self-hosted box TEMP happens to resolve
        // under the profile and inherits, but on a hosted runner it is a
        // separate volume that does not — which is a property of the runner,
        // not of the code under test.
        #[cfg(windows)]
        let tmp = tempfile::Builder::new()
            .tempdir_in(std::env::var("USERPROFILE").expect("USERPROFILE is set on Windows"))
            .unwrap();
        #[cfg(not(windows))]
        let tmp = tempfile::tempdir().unwrap();

        let path = tmp.path().join(".secret.key");
        std::fs::write(&path, [7u8; 32]).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(load_or_create_key(&path).unwrap(), [7u8; 32]);
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "a loose pre-existing key must be tightened"
            );
        }
        #[cfg(windows)]
        {
            let acl_of = |p: &std::path::Path| {
                let out = std::process::Command::new("icacls")
                    .arg(p)
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&out.stdout).into_owned()
            };
            assert!(
                acl_of(&path).contains("(I)"),
                "fixture precondition: a fresh file under the profile inherits ACEs"
            );
            assert_eq!(load_or_create_key(&path).unwrap(), [7u8; 32]);
            assert!(
                !acl_of(&path).contains("(I)"),
                "a pre-existing key must lose its inherited ACEs on load"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn key_file_has_no_inherited_acl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".secret.key");
        load_or_create_key(&path).unwrap();

        let out = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .expect("icacls is present on every supported Windows");
        let acl = String::from_utf8_lossy(&out.stdout);
        assert!(
            !acl.contains("(I)"),
            "key file still carries inherited ACEs:\n{acl}"
        );
        assert!(
            acl.contains(&std::env::var("USERNAME").unwrap()),
            "key file must stay readable by its owner:\n{acl}"
        );
    }
}
