//! Parameter admission only, not cryptography. Borrow DER integers or SSH
//! mpints before any RSA Montgomery/precomputation or signing work begins.

use super::ImportError;
use pkcs8::der::Decode as _;
use russh::keys::{
    ssh_key::{Algorithm, Cipher, EcdsaCurve},
    PrivateKey,
};

const MAX_RSA_BYTES: usize = 1024; // 8192 bits, matching rsa's default public-key maximum.

fn positive(bytes: &[u8]) -> &[u8] {
    let first = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    &bytes[first..]
}

pub(crate) fn check_rsa_public(n: &[u8], e: &[u8]) -> Result<(), ImportError> {
    let n = positive(n);
    let e = positive(e);
    if n.is_empty()
        || n.len() > MAX_RSA_BYTES
        || e.is_empty()
        || e.len() > 5
        || (e.len() == 5 && e[0] > 1)
    {
        return Err(ImportError::Parameters);
    }
    Ok(())
}

fn supported_algorithm(algorithm: Algorithm) -> bool {
    // Match the product's locked ssh-key signing features, not just what its
    // container parser can represent. In particular DSA/FIDO/Other can parse
    // without a signing implementation; each ECDSA curve is enabled explicitly.
    matches!(
        algorithm,
        Algorithm::Ed25519
            | Algorithm::Rsa { .. }
            | Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256
            }
            | Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384
            }
            | Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP521
            }
    )
}

pub(super) fn supported_openssh_cipher(cipher: Cipher) -> bool {
    // ssh-cipher parses TripleDES too, but its tdes feature is not enabled.
    matches!(
        cipher,
        Cipher::None
            | Cipher::Aes128Cbc
            | Cipher::Aes192Cbc
            | Cipher::Aes256Cbc
            | Cipher::Aes128Ctr
            | Cipher::Aes192Ctr
            | Cipher::Aes256Ctr
            | Cipher::Aes128Gcm
            | Cipher::Aes256Gcm
            | Cipher::ChaCha20Poly1305
    )
}

pub(crate) fn check_key_parameters(key: &PrivateKey) -> Result<(), ImportError> {
    if !supported_algorithm(key.algorithm()) {
        return Err(ImportError::UnsupportedAlgorithm);
    }
    if let Some(public) = key.public_key().key_data().rsa() {
        check_rsa_public(
            public
                .n()
                .as_positive_bytes()
                .ok_or(ImportError::Parameters)?,
            public
                .e()
                .as_positive_bytes()
                .ok_or(ImportError::Parameters)?,
        )?;
    }
    if let Some(pair) = key.key_data().rsa() {
        let private = pair.private();
        for field in [private.d(), private.p(), private.q(), private.iqmp()] {
            let bytes = field.as_positive_bytes().ok_or(ImportError::Parameters)?;
            if bytes.is_empty() || bytes.len() > MAX_RSA_BYTES {
                return Err(ImportError::Parameters);
            }
        }
    }
    Ok(())
}

fn check_pkcs1_parameters(bytes: &[u8]) -> Result<(), ImportError> {
    let key = pkcs1::RsaPrivateKey::from_der(bytes).map_err(|_| ImportError::Invalid)?;
    check_rsa_public(key.modulus.as_bytes(), key.public_exponent.as_bytes())?;
    for field in [
        key.private_exponent,
        key.prime1,
        key.prime2,
        key.exponent1,
        key.exponent2,
        key.coefficient,
    ] {
        if field.as_bytes().is_empty() || field.as_bytes().len() > MAX_RSA_BYTES {
            return Err(ImportError::Parameters);
        }
    }
    // The production decoder only uses the two-prime form. Do not silently
    // discard extra prime factors from an imported key.
    if key.other_prime_infos.is_some() {
        return Err(ImportError::Parameters);
    }
    Ok(())
}

pub(super) fn check_pkcs8_parameters(bytes: &[u8]) -> Result<(), ImportError> {
    let key = pkcs8::PrivateKeyInfoRef::from_der(bytes).map_err(|_| ImportError::Invalid)?;
    if key.algorithm.oid == pkcs1::ALGORITHM_OID {
        check_pkcs1_parameters(key.private_key.as_bytes())?;
    }
    Ok(())
}

pub(super) fn check_pem_parameters(text: &str) -> Result<(), ImportError> {
    let (_, document) = pkcs8::SecretDocument::from_pem(text).map_err(|_| ImportError::Invalid)?;
    // PEM labels are not algorithm evidence. russh's ASN.1 decoder tries
    // multiple containers, so check every RSA structure it can consume before
    // giving it any accepted label (including a relabelled EC PRIVATE KEY).
    let bytes = document.as_bytes();
    if pkcs8::PrivateKeyInfoRef::from_der(bytes).is_ok() {
        check_pkcs8_parameters(bytes)?;
    } else if pkcs1::RsaPrivateKey::from_der(bytes).is_ok() {
        check_pkcs1_parameters(bytes)?;
    }
    // A non-RSA container still has to pass the official full decoder; this
    // function only admits parameters and never calls a key cryptographically valid.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_small_synthetic_boundary_crossings_before_big_integer_work() {
        assert_eq!(
            check_rsa_public(&vec![1; MAX_RSA_BYTES], &[1, 0, 1]),
            Ok(())
        );
        assert_eq!(
            check_rsa_public(&vec![1; MAX_RSA_BYTES + 1], &[1, 0, 1]),
            Err(ImportError::Parameters)
        );
        assert_eq!(
            check_rsa_public(&[7], &[2, 0, 0, 0, 0]),
            Err(ImportError::Parameters)
        );
        assert_eq!(check_rsa_public(&[7], &[1, 0, 0, 0, 0]), Ok(()));
        assert_eq!(check_rsa_public(&[], &[3]), Err(ImportError::Parameters));
    }
    #[test]
    fn rsa_parameter_admission_uses_der_not_the_pem_label() {
        use pkcs8::der::{
            asn1::{OctetStringRef, UintRef},
            Encode as _,
        };
        // Just 1025 bytes: this fixture is never converted to a big integer,
        // and intentionally is not a cryptographically valid RSA key.
        let oversized = [1; MAX_RSA_BYTES + 1];
        let one = UintRef::new(&[1]).unwrap();
        let rsa = pkcs1::RsaPrivateKey {
            modulus: UintRef::new(&oversized).unwrap(),
            public_exponent: UintRef::new(&[1, 0, 1]).unwrap(),
            private_exponent: one,
            prime1: one,
            prime2: one,
            exponent1: one,
            exponent2: one,
            coefficient: one,
            other_prime_infos: None,
        }
        .to_der()
        .unwrap();
        let info =
            pkcs8::PrivateKeyInfoRef::new(pkcs1::ALGORITHM_ID, OctetStringRef::new(&rsa).unwrap());
        let pkcs8 = info.to_der().unwrap();
        for bytes in [&rsa, &pkcs8] {
            for label in ["RSA PRIVATE KEY", "PRIVATE KEY", "EC PRIVATE KEY"] {
                let pem = pkcs8::SecretDocument::try_from(bytes.as_slice())
                    .unwrap()
                    .to_pem(label, pkcs8::LineEnding::LF)
                    .unwrap();
                assert_eq!(check_pem_parameters(&pem), Err(ImportError::Parameters));
                assert_eq!(
                    super::super::import_key(zeroize::Zeroizing::new(pem.to_string())).err(),
                    Some(ImportError::Parameters)
                );
            }
        }
    }
    #[test]
    fn backend_support_is_narrower_than_parseable_algorithm_names() {
        for algorithm in [
            Algorithm::Ed25519,
            Algorithm::Rsa { hash: None },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP521,
            },
        ] {
            assert!(supported_algorithm(algorithm));
        }
        for algorithm in [
            Algorithm::Dsa,
            Algorithm::SkEd25519,
            Algorithm::SkEcdsaSha2NistP256,
            Algorithm::new("synthetic-unsupported@example.invalid").unwrap(),
        ] {
            assert!(!supported_algorithm(algorithm));
        }
        assert!(!supported_openssh_cipher(Cipher::TDesCbc));
    }
}
