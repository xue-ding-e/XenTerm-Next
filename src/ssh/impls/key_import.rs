//! Bounded, in-memory private-key imports for the session editor.
//! Validation never writes a key, derives an encrypted key, or exposes parser errors.

use std::io::Read;
use std::path::Path;

use russh::keys::{decode_secret_key, ssh_key::Kdf, PrivateKey};
use zeroize::Zeroizing;

#[path = "key_import_bounds.rs"]
mod bounds;
pub(crate) use bounds::{check_key_parameters, check_rsa_public};

pub(crate) const MAX_IMPORT_BYTES: usize = 1024 * 1024;

/// One admission pool for import inspection and key decryption. A cancelled synchronous operation keeps its permit until it
/// actually returns; repeated clicks cannot accumulate abandoned workers.
pub(crate) async fn work_permit() -> Result<tokio::sync::OwnedSemaphorePermit, ImportError> {
    use std::sync::{Arc, OnceLock};
    static WORKERS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    WORKERS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ImportError::File)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportError {
    File,
    TooLarge,
    Invalid,
    UnsupportedEncryption,
    UnsupportedAlgorithm,
    Parameters,
}

pub(crate) struct ImportedKey {
    pub(crate) content: Zeroizing<String>,
    /// The container is valid; its passphrase is checked only when connecting.
    pub(crate) encrypted: bool,
}

pub(crate) fn read_key_file(path: &Path) -> Result<ImportedKey, ImportError> {
    #[cfg(windows)]
    check_local_windows_path(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ImportError::File)?;
    // Do not follow dropped symlinks or read directories/devices/FIFOs.
    if !metadata.file_type().is_file() {
        return Err(ImportError::File);
    }
    if metadata.len() > MAX_IMPORT_BYTES as u64 {
        return Err(ImportError::TooLarge);
    }
    let file = open_key_file(path, &metadata)?;
    let opened = file.metadata().map_err(|_| ImportError::File)?;
    if !opened.is_file() {
        return Err(ImportError::File);
    }
    // The extra byte detects a file which grew after the metadata check.
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_IMPORT_BYTES + 1));
    file.take(MAX_IMPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError::File)?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::TooLarge);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ImportError::Invalid)?;
    import_key(Zeroizing::new(text.to_owned()))
}

#[cfg(unix)]
fn open_key_file(path: &Path, expected: &std::fs::Metadata) -> Result<std::fs::File, ImportError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ImportError::File)?;
    let opened = file.metadata().map_err(|_| ImportError::File)?;
    if !opened.is_file() || opened.dev() != expected.dev() || opened.ino() != expected.ino() {
        return Err(ImportError::File);
    }
    Ok(file)
}

#[cfg(windows)]
fn check_local_windows_path(path: &Path) -> Result<(), ImportError> {
    use std::path::{Component, Prefix};
    // Check before even asking for metadata: UNC/device namespaces can cause
    // network access or pipe operations, rather than a local regular-file read.
    if matches!(path.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    {
        Ok(())
    } else {
        Err(ImportError::File)
    }
}

#[cfg(windows)]
fn open_key_file(path: &Path, _: &std::fs::Metadata) -> Result<std::fs::File, ImportError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, GetFileType, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_TYPE_DISK,
    };
    check_local_windows_path(path)?;
    let open = || {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .map_err(|_| ImportError::File)
    };
    let file = open()?;
    let held = file.metadata().map_err(|_| ImportError::File)?;
    if !held.is_file()
        || held.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK
    {
        return Err(ImportError::File);
    }
    let identity = |file: &std::fs::File| {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(|_| ImportError::File)?;
        Ok::<_, ImportError>((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    };
    // A read-only sharing mode denies replacement while this handle is held.
    // Reopen without following a reparse point and verify the pathname still
    // names the held file; all reads below use only that original handle.
    if identity(&file)? != identity(&open()?)? {
        return Err(ImportError::File);
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_key_file(_: &Path, _: &std::fs::Metadata) -> Result<std::fs::File, ImportError> {
    Err(ImportError::File)
}

pub(crate) fn import_key(content: Zeroizing<String>) -> Result<ImportedKey, ImportError> {
    if content.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::TooLarge);
    }
    let text = content.trim();
    if text.is_empty() || text.contains('\0') {
        return Err(ImportError::Invalid);
    }
    let encrypted = if text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
        let key = PrivateKey::from_openssh(text).map_err(|_| ImportError::Invalid)?;
        check_key_parameters(&key)?;
        if !bounds::supported_openssh_cipher(key.cipher()) {
            return Err(ImportError::UnsupportedAlgorithm);
        }
        // Refuse excessive KDF work before import can enable a later job.
        // The normal OpenSSH default is 16 rounds.
        match key.kdf() {
            Kdf::None => {}
            Kdf::Bcrypt { rounds, .. } if (1..=128).contains(rounds) => {}
            _ => return Err(ImportError::UnsupportedEncryption),
        }
        key.is_encrypted()
    } else if super::ppk::is_ppk(text.as_bytes()) {
        super::ppk::validate_import(text.as_bytes()).map_err(|_| ImportError::Invalid)?
    } else if text.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        let (label, document) =
            pkcs8::SecretDocument::from_pem(text).map_err(|_| ImportError::Invalid)?;
        if label != "ENCRYPTED PRIVATE KEY" {
            return Err(ImportError::Invalid);
        }
        let container = document
            .decode_msg::<pkcs8::EncryptedPrivateKeyInfoRef<'_>>()
            .map_err(|_| ImportError::Invalid)?;
        if container.encrypted_data.as_bytes().is_empty() {
            return Err(ImportError::Invalid);
        }
        if !bounded_pkcs8_kdf(&container.encryption_algorithm) {
            return Err(ImportError::UnsupportedEncryption);
        }
        true
    } else {
        if ![
            "-----BEGIN PRIVATE KEY-----",
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----BEGIN EC PRIVATE KEY-----",
        ]
        .iter()
        .any(|header| text.starts_with(header))
        {
            return Err(ImportError::Invalid);
        }
        if text.contains("Proc-Type: 4,ENCRYPTED") {
            return Err(ImportError::UnsupportedEncryption);
        } else {
            bounds::check_pem_parameters(text)?;
        }
        match decode_secret_key(text, None) {
            Ok(_) => false,
            Err(russh::keys::Error::KeyIsEncrypted) => {
                return Err(ImportError::UnsupportedEncryption)
            }
            Err(_) => return Err(ImportError::Invalid),
        }
    };
    Ok(ImportedKey { content, encrypted })
}

/// Shared decryption entry for imported inline keys. All KDF parameters are admitted by
/// import_key, and RSA integers are checked before conversion/signing. No
/// cryptography is implemented here; all decoding/decryption is delegated to
/// the same locked RustCrypto/russh parsers used by ordinary connections.
#[cfg(test)]
fn decode_imported_key(text: &str, passphrase: &str) -> Result<PrivateKey, ImportError> {
    let admitted = import_key(Zeroizing::new(text.to_owned()))?;
    decode_admitted_key(&admitted, passphrase)
}

/// Older profiles may contain text accepted by russh that the new editor would
/// decline to import. Preserve the existing decoder for those saved values.
pub(crate) fn decode_saved_inline(
    text: &str,
    passphrase: &str,
) -> anyhow::Result<PrivateKey> {
    use anyhow::Context as _;

    let text = text.trim();
    let admission = if text.len() <= MAX_IMPORT_BYTES {
        import_key(Zeroizing::new(text.to_owned()))
    } else {
        Err(ImportError::TooLarge)
    };
    match admission {
        Ok(admitted) => decode_admitted_key(&admitted, passphrase).map_err(|_| {
            anyhow::anyhow!("private key format, parameters or passphrase are invalid or unsupported")
        }),
        Err(_) if super::ppk::is_ppk(text.as_bytes()) => {
            super::ppk::decode_ppk(text.as_bytes(), passphrase)
                .context("failed to parse saved PuTTY private key")
        }
        Err(_) => decode_secret_key(text, (!passphrase.is_empty()).then_some(passphrase))
            .context("failed to parse saved private key"),
    }
}

fn decode_admitted_key(
    admitted: &ImportedKey,
    passphrase: &str,
) -> Result<PrivateKey, ImportError> {
    let text = admitted.content.trim();
    let key = if text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
        let key = PrivateKey::from_openssh(text).map_err(|_| ImportError::Invalid)?;
        if key.is_encrypted() {
            key.decrypt(passphrase).map_err(|_| ImportError::Invalid)?
        } else {
            key
        }
    } else if super::ppk::is_ppk(text.as_bytes()) {
        super::ppk::decode_ppk(text.as_bytes(), passphrase).map_err(|_| ImportError::Invalid)?
    } else if text.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        let (_, encrypted) =
            pkcs8::SecretDocument::from_pem(text).map_err(|_| ImportError::Invalid)?;
        let container = encrypted
            .decode_msg::<pkcs8::EncryptedPrivateKeyInfoRef<'_>>()
            .map_err(|_| ImportError::Invalid)?;
        let plain = container
            .decrypt(passphrase)
            .map_err(|_| ImportError::Invalid)?;
        bounds::check_pkcs8_parameters(plain.as_bytes())?;
        russh::keys::pkcs8::decode_pkcs8(plain.as_bytes(), None)
            .map_err(|_| ImportError::Invalid)?
    } else {
        // Unencrypted PEM was bounded by import_key, before its first conversion.
        decode_secret_key(text, None).map_err(|_| ImportError::Invalid)?
    };
    check_key_parameters(&key)?;
    Ok(key)
}

fn bounded_pkcs8_kdf(scheme: &pkcs8::pkcs5::EncryptionScheme) -> bool {
    use pkcs8::pkcs5::{pbes2::Kdf, EncryptionScheme};
    let EncryptionScheme::Pbes2(parameters) = scheme else {
        return false;
    };
    // Accept only ciphers enabled by the already locked decoder, and require
    // an explicit derived-key length to match that cipher. SHA-1 PBKDF2 is
    // parsed by pkcs5 but is not enabled by its current feature set.
    let key_size = match parameters.encryption {
        pkcs8::pkcs5::pbes2::EncryptionScheme::Aes128Cbc { .. }
        | pkcs8::pkcs5::pbes2::EncryptionScheme::Aes128Gcm { .. } => 16,
        pkcs8::pkcs5::pbes2::EncryptionScheme::Aes192Cbc { .. } => 24,
        pkcs8::pkcs5::pbes2::EncryptionScheme::Aes256Cbc { .. }
        | pkcs8::pkcs5::pbes2::EncryptionScheme::Aes256Gcm { .. } => 32,
        _ => return false,
    };
    match &parameters.kdf {
        Kdf::Pbkdf2(params) => {
            (1..=1_000_000).contains(&params.iteration_count)
                && !matches!(params.prf, pkcs8::pkcs5::pbes2::Pbkdf2Prf::HmacWithSha1)
                && params.key_length.is_none_or(|length| length == key_size)
        }
        Kdf::Scrypt(params) => {
            let n = params.cost_parameter;
            let r = u64::from(params.block_size);
            let p = u64::from(params.parallelization);
            n > 1
                && n.is_power_of_two()
                && n <= (1 << 18)
                && (1..=8).contains(&r)
                && (1..=4).contains(&p)
                && n.saturating_mul(r).saturating_mul(128) <= 256 * 1024 * 1024
                && n.saturating_mul(r).saturating_mul(p) <= (1 << 22)
                && params.key_length.is_none_or(|length| length == key_size)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn saved_inline_keeps_russh_compatible_pem_with_a_leading_comment() {
        let key = include_str!("../../../tests/fixtures/import-synthetic-ec-sec1.pem");
        let with_comment = format!("Synthetic file annotation\n{key}");
        assert!(import_key(Zeroizing::new(with_comment.clone())).is_err());
        let expected = decode_saved_inline(key, "").unwrap();
        let old_profile = decode_saved_inline(&with_comment, "").unwrap();
        assert_eq!(expected.public_key(), old_profile.public_key());
    }

    use super::*;

    const PPK: &str = include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk");
    const ENCRYPTED: &str = include_str!("../../../tests/fixtures/ppk-v3-ecdsa-encrypted.ppk");

    #[test]
    fn validates_plain_and_encrypted_synthetic_containers_without_decryption() {
        let plain = import_key(Zeroizing::new(PPK.into())).unwrap();
        assert!(!plain.encrypted);
        assert_eq!(plain.content.as_str(), PPK);
        assert!(
            import_key(Zeroizing::new(ENCRYPTED.into()))
                .unwrap()
                .encrypted
        );
    }

    #[test]
    fn rejects_public_keys_certificates_malformed_text_and_oversized_content() {
        for text in [
            "",
            "ssh-ed25519 synthetic-public-key",
            "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----",
            "-----BEGIN OPENSSH PRIVATE KEY-----\ninvalid",
            "secret\0value",
        ] {
            assert_eq!(
                import_key(Zeroizing::new(text.into())).err(),
                Some(ImportError::Invalid)
            );
        }
        assert_eq!(
            import_key(Zeroizing::new("x".repeat(MAX_IMPORT_BYTES + 1))).err(),
            Some(ImportError::TooLarge)
        );
        assert_eq!(
            import_key(Zeroizing::new(
                "-----BEGIN ENCRYPTED PRIVATE KEY-----\n".into()
            ))
            .err(),
            Some(ImportError::Invalid)
        );
    }

    #[test]
    fn file_import_checks_regular_file_size_and_utf8() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_key_file(dir.path()).err(), Some(ImportError::File));
        let path = dir.path().join("synthetic.ppk");
        std::fs::write(&path, PPK).unwrap();
        assert_eq!(read_key_file(&path).unwrap().content.as_str(), PPK);
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(read_key_file(&path).err(), Some(ImportError::Invalid));
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_IMPORT_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(read_key_file(&path).err(), Some(ImportError::TooLarge));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_without_reading_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("synthetic-link");
        std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
        assert_eq!(read_key_file(&link).err(), Some(ImportError::File));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_replacement_file_symlink_and_fifo_after_metadata_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.ppk");
        std::fs::write(&path, PPK).unwrap();
        let expected = std::fs::symlink_metadata(&path).unwrap();
        let old = dir.path().join("old.ppk");
        std::fs::rename(&path, &old).unwrap();
        std::fs::write(&path, PPK).unwrap();
        assert!(matches!(
            open_key_file(&path, &expected),
            Err(ImportError::File)
        ));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&old, &path).unwrap();
        assert!(matches!(
            open_key_file(&path, &expected),
            Err(ImportError::File)
        ));
        std::fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // O_NONBLOCK ensures this cannot wait for a writer before metadata rejects it.
        assert!(matches!(
            open_key_file(&path, &expected),
            Err(ImportError::File)
        ));
    }

    #[test]
    fn accepts_synthetic_openssh_and_pkcs8_pem() {
        let key = super::super::ppk::decode_ppk(PPK.as_bytes(), "").unwrap();
        let openssh = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        assert!(
            !import_key(Zeroizing::new(openssh.to_string()))
                .unwrap()
                .encrypted
        );
        // Deterministic Ed25519 seed [7; 32], encoded as a PKCS#8 document.
        use base64::Engine as _;
        let mut der = vec![
            0x30, 0x2e, 2, 1, 0, 0x30, 5, 6, 3, 0x2b, 0x65, 0x70, 4, 0x22, 4, 0x20,
        ];
        der.extend([7; 32]);
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(der)
        );
        assert!(!import_key(Zeroizing::new(pem)).unwrap().encrypted);
    }

    #[test]
    fn imports_encrypted_pkcs8_without_a_passphrase_then_checks_it_at_use() {
        use pkcs8::der::Decode as _;
        let mut der = vec![
            0x30, 0x2e, 2, 1, 0, 0x30, 5, 6, 3, 0x2b, 0x65, 0x70, 4, 0x22, 4, 0x20,
        ];
        der.extend([7; 32]);
        let key = pkcs8::PrivateKeyInfoRef::from_der(&der).unwrap();
        let params = pkcs8::pkcs5::pbes2::Parameters::generate_pbkdf2_sha256_aes256cbc(
            16, &[9; 16], [11; 16],
        )
        .unwrap();
        let encrypted = key
            .encrypt_with_params(params, "synthetic-passphrase")
            .unwrap();
        let pem = encrypted
            .to_pem("ENCRYPTED PRIVATE KEY", pkcs8::LineEnding::LF)
            .unwrap();
        assert!(
            import_key(Zeroizing::new(pem.to_string()))
                .unwrap()
                .encrypted
        );
        assert!(decode_imported_key(&pem, "wrong-synthetic-passphrase").is_err());
        assert!(decode_imported_key(&pem, "synthetic-passphrase").is_ok());
    }

    #[test]
    fn every_admitted_pkcs8_cipher_and_kdf_family_decrypts_with_locked_features() {
        use pkcs8::{
            der::Decode as _,
            pkcs5::pbes2::{
                EncryptionScheme, Kdf, Parameters, Pbkdf2Params, Pbkdf2Prf, ScryptParams,
            },
        };
        let der = russh::keys::pkcs8::encode_pkcs8(
            &super::super::ppk::decode_ppk(PPK.as_bytes(), "").unwrap(),
        )
        .unwrap();
        let key = pkcs8::PrivateKeyInfoRef::from_der(&der).unwrap();
        let mut kdfs = Vec::new();
        for prf in [
            Pbkdf2Prf::HmacWithSha224,
            Pbkdf2Prf::HmacWithSha256,
            Pbkdf2Prf::HmacWithSha384,
            Pbkdf2Prf::HmacWithSha512,
        ] {
            let mut params = Pbkdf2Params::hmac_sha256(2, &[9; 16]).unwrap();
            params.prf = prf;
            kdfs.push(Kdf::Pbkdf2(params));
        }
        kdfs.push(Kdf::Scrypt(ScryptParams {
            salt: (&[9u8; 16][..]).try_into().unwrap(),
            cost_parameter: 16,
            block_size: 1,
            parallelization: 1,
            key_length: None,
        }));
        // Tiny deterministic public fixtures only: no production-work-factor KDF.
        for kdf in kdfs {
            for encryption in [
                EncryptionScheme::Aes128Cbc { iv: [11; 16] },
                EncryptionScheme::Aes192Cbc { iv: [11; 16] },
                EncryptionScheme::Aes256Cbc { iv: [11; 16] },
                EncryptionScheme::Aes128Gcm { nonce: [11; 12] },
                EncryptionScheme::Aes256Gcm { nonce: [11; 12] },
            ] {
                let parameters = Parameters {
                    kdf: kdf.clone(),
                    encryption,
                };
                let encrypted = key
                    .encrypt_with_params(parameters, "synthetic-family-passphrase")
                    .unwrap();
                let pem = encrypted
                    .to_pem("ENCRYPTED PRIVATE KEY", pkcs8::LineEnding::LF)
                    .unwrap();
                assert!(
                    import_key(Zeroizing::new(pem.to_string()))
                        .unwrap()
                        .encrypted
                );
                assert!(decode_imported_key(&pem, "synthetic-family-passphrase").is_ok());
            }
        }
    }

    #[test]
    fn accepts_generated_synthetic_sec1_ec_fixture() {
        // Generated for this test with `openssl ecparam -name <named-curve> -genkey -noout`.
        // Never an account credential; this public fixture must not be used for authentication.
        for pem in [
            include_str!("../../../tests/fixtures/import-synthetic-ec-sec1.pem"),
            include_str!("../../../tests/fixtures/import-synthetic-ec-p384-sec1.pem"),
            include_str!("../../../tests/fixtures/import-synthetic-ec-p521-sec1.pem"),
        ] {
            assert!(!import_key(Zeroizing::new(pem.into())).unwrap().encrypted);
            assert!(decode_imported_key(pem, "").is_ok());
        }
    }

    #[test]
    fn rejects_parseable_hardware_key_without_a_signing_backend() {
        use russh::keys::ssh_key::{private, public, LineEnding};
        let plain = super::super::ppk::decode_ppk(PPK.as_bytes(), "").unwrap();
        let public = public::SkEd25519::new(
            *plain.public_key().key_data().ed25519().unwrap(),
            "ssh:synthetic-fixture",
        );
        let hardware = private::SkEd25519::new(public, 0, vec![7]).unwrap();
        let key = PrivateKey::new(hardware.into(), "synthetic unsupported hardware key").unwrap();
        let pem = key.to_openssh(LineEnding::LF).unwrap();
        assert!(
            PrivateKey::from_openssh(&pem).is_ok(),
            "fixture must remain parseable"
        );
        assert_eq!(
            import_key(Zeroizing::new(pem.to_string())).err(),
            Some(ImportError::UnsupportedAlgorithm)
        );
    }

    #[test]
    fn every_enabled_openssh_cipher_family_round_trips() {
        use russh::keys::ssh_key::{Cipher, LineEnding};
        let plain = super::super::ppk::decode_ppk(PPK.as_bytes(), "").unwrap();
        for cipher in [
            Cipher::Aes128Cbc,
            Cipher::Aes192Cbc,
            Cipher::Aes256Cbc,
            Cipher::Aes128Ctr,
            Cipher::Aes192Ctr,
            Cipher::Aes256Ctr,
            Cipher::Aes128Gcm,
            Cipher::Aes256Gcm,
            Cipher::ChaCha20Poly1305,
        ] {
            let encrypted = plain
                .encrypt_with(
                    cipher,
                    Kdf::Bcrypt {
                        salt: vec![9; 16],
                        rounds: 1,
                    },
                    7,
                    "synthetic-cipher-passphrase",
                )
                .unwrap();
            let pem = encrypted.to_openssh(LineEnding::LF).unwrap();
            assert!(
                import_key(Zeroizing::new(pem.to_string()))
                    .unwrap()
                    .encrypted
            );
            assert!(decode_imported_key(&pem, "synthetic-cipher-passphrase").is_ok());
        }
    }

    #[test]
    fn synthetic_rsa_pkcs1_pkcs8_and_openssh_use_the_same_bounds() {
        use pkcs8::der::Decode as _;
        let ppk = include_str!("../../../tests/fixtures/ppk-v2-rsa.ppk");
        let key = super::super::ppk::decode_ppk(ppk.as_bytes(), "").unwrap();
        let der = russh::keys::pkcs8::encode_pkcs8(&key).unwrap();
        let info = pkcs8::PrivateKeyInfoRef::from_der(&der).unwrap();
        for (label, bytes) in [
            ("PRIVATE KEY", der.as_slice()),
            ("RSA PRIVATE KEY", info.private_key.as_bytes()),
        ] {
            let pem = pkcs8::SecretDocument::try_from(bytes)
                .unwrap()
                .to_pem(label, pkcs8::LineEnding::LF)
                .unwrap();
            assert!(
                !import_key(Zeroizing::new(pem.to_string()))
                    .unwrap()
                    .encrypted
            );
            assert!(decode_imported_key(&pem, "").is_ok());
        }
        let openssh = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        assert!(decode_imported_key(&openssh, "").is_ok());
    }

    #[test]
    fn supported_encrypted_openssh_and_ppk_fixtures_decrypt_only_at_use() {
        let key = super::super::ppk::decode_ppk(PPK.as_bytes(), "").unwrap();
        let encrypted = key
            .encrypt_with(
                russh::keys::ssh_key::Cipher::Aes256Ctr,
                Kdf::Bcrypt {
                    salt: vec![9; 16],
                    rounds: 1,
                },
                7,
                "synthetic-passphrase",
            )
            .unwrap();
        let pem = encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        assert!(
            import_key(Zeroizing::new(pem.to_string()))
                .unwrap()
                .encrypted
        );
        assert!(decode_imported_key(&pem, "synthetic-passphrase").is_ok());
        for (text, passphrase) in [
            (
                include_str!("../../../tests/fixtures/ppk-v2-rsa-encrypted.ppk"),
                "v2-passphrase",
            ),
            (ENCRYPTED, "test-passphrase"),
        ] {
            assert!(import_key(Zeroizing::new(text.into())).unwrap().encrypted);
            assert!(decode_imported_key(text, passphrase).is_ok());
        }
    }

    #[test]
    fn rejects_kdf_boundary_excess_without_running_the_kdf() {
        let mut params = pkcs8::pkcs5::pbes2::Parameters::generate_pbkdf2_sha256_aes256cbc(
            1_000_000, &[9; 16], [11; 16],
        )
        .unwrap();
        assert!(bounded_pkcs8_kdf(&params.clone().into()));
        if let pkcs8::pkcs5::pbes2::Kdf::Pbkdf2(parameters) = &mut params.kdf {
            parameters.iteration_count += 1;
        }
        assert!(!bounded_pkcs8_kdf(&params.into()));
    }

    #[test]
    fn legacy_encrypted_pem_is_rejected_without_decrypting_or_replacing_a_key() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00000000000000000000000000000000\n\nAA==\n-----END RSA PRIVATE KEY-----\n";
        assert_eq!(
            import_key(Zeroizing::new(pem.into())).err(),
            Some(ImportError::UnsupportedEncryption)
        );
    }

    #[tokio::test]
    async fn cancelled_blocking_work_keeps_admission_until_it_actually_finishes() {
        let first = work_permit().await.unwrap();
        let second = work_permit().await.unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = first;
            let _ = started.send(());
            wait.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
        });
        ready.await.unwrap();
        work.abort();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), work_permit())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        work.await.unwrap();
        let admitted = tokio::time::timeout(std::time::Duration::from_secs(1), work_permit())
            .await
            .unwrap()
            .unwrap();
        drop((admitted, second));
    }
}
