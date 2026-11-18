// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

//!
//! This module exports cipher names for use with [Preferred].
use std::collections::HashMap;
use std::convert::TryFrom;
use std::marker::PhantomData;
use std::sync::LazyLock;

use delegate::delegate;
use digest::typenum::{U16, U20, U32, U64};
use hmac::Hmac;
use md5_compat::Md5;
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use ssh_encoding::Encode;

use self::crypto::CryptoMacAlgorithm;
use self::crypto_etm::CryptoEtmMacAlgorithm;
use self::none::NoMacAlgorithm;

mod crypto;
mod crypto_etm;
mod none;

pub(crate) trait MacAlgorithm {
    fn key_len(&self) -> usize;
    fn make_mac(&self, key: &[u8]) -> Box<dyn Mac + Send>;
}

pub(crate) trait Mac {
    fn mac_len(&self) -> usize;
    fn is_etm(&self) -> bool {
        false
    }
    fn compute(&self, sequence_number: u32, payload: &[u8], output: &mut [u8]);
    fn verify(&self, sequence_number: u32, payload: &[u8], mac: &[u8]) -> bool;
}

#[derive(Debug, PartialEq, Eq, Copy, Clone, Hash)]
pub struct Name(&'static str);
impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.0
    }
}

impl Encode for Name {
    delegate! { to self.as_ref() {
        fn encoded_len(&self) -> Result<usize, ssh_encoding::Error>;
        fn encode(&self, writer: &mut impl ssh_encoding::Writer) -> Result<(), ssh_encoding::Error>;
    }}
}

impl TryFrom<&str> for Name {
    type Error = ();
    fn try_from(s: &str) -> Result<Name, ()> {
        MACS.keys().find(|x| x.0 == s).map(|x| **x).ok_or(())
    }
}

/// `none`
pub const NONE: Name = Name("none");
/// Full-length legacy MAC; applications must opt in explicitly.
pub const HMAC_MD5: Name = Name("hmac-md5");
/// `hmac-sha1`
pub const HMAC_SHA1: Name = Name("hmac-sha1");
/// `hmac-sha2-256`
pub const HMAC_SHA256: Name = Name("hmac-sha2-256");
/// `hmac-sha2-512`
pub const HMAC_SHA512: Name = Name("hmac-sha2-512");
/// `hmac-sha1-etm@openssh.com`
pub const HMAC_SHA1_ETM: Name = Name("hmac-sha1-etm@openssh.com");
/// `hmac-sha2-256-etm@openssh.com`
pub const HMAC_SHA256_ETM: Name = Name("hmac-sha2-256-etm@openssh.com");
/// `hmac-sha2-512-etm@openssh.com`
pub const HMAC_SHA512_ETM: Name = Name("hmac-sha2-512-etm@openssh.com");

pub(crate) static _HMAC_MD5: CryptoMacAlgorithm<Hmac<Md5>, U16> =
    CryptoMacAlgorithm(PhantomData, PhantomData);
pub(crate) static _NONE: NoMacAlgorithm = NoMacAlgorithm {};
pub(crate) static _HMAC_SHA1: CryptoMacAlgorithm<Hmac<Sha1>, U20> =
    CryptoMacAlgorithm(PhantomData, PhantomData);
pub(crate) static _HMAC_SHA256: CryptoMacAlgorithm<Hmac<Sha256>, U32> =
    CryptoMacAlgorithm(PhantomData, PhantomData);
pub(crate) static _HMAC_SHA512: CryptoMacAlgorithm<Hmac<Sha512>, U64> =
    CryptoMacAlgorithm(PhantomData, PhantomData);
pub(crate) static _HMAC_SHA1_ETM: CryptoEtmMacAlgorithm<Hmac<Sha1>, U20> =
    CryptoEtmMacAlgorithm(PhantomData, PhantomData);
pub(crate) static _HMAC_SHA256_ETM: CryptoEtmMacAlgorithm<Hmac<Sha256>, U32> =
    CryptoEtmMacAlgorithm(PhantomData, PhantomData);
pub(crate) static _HMAC_SHA512_ETM: CryptoEtmMacAlgorithm<Hmac<Sha512>, U64> =
    CryptoEtmMacAlgorithm(PhantomData, PhantomData);

pub const ALL_MAC_ALGORITHMS: &[&Name] = &[
    &NONE,
    &HMAC_MD5,
    &HMAC_SHA1,
    &HMAC_SHA256,
    &HMAC_SHA512,
    &HMAC_SHA1_ETM,
    &HMAC_SHA256_ETM,
    &HMAC_SHA512_ETM,
];

pub(crate) static MACS: LazyLock<HashMap<&'static Name, &(dyn MacAlgorithm + Send + Sync)>> =
    LazyLock::new(|| {
        let mut h: HashMap<&'static Name, &(dyn MacAlgorithm + Send + Sync)> = HashMap::new();
        h.insert(&NONE, &_NONE);
        h.insert(&HMAC_MD5, &_HMAC_MD5);
        h.insert(&HMAC_SHA1, &_HMAC_SHA1);
        h.insert(&HMAC_SHA256, &_HMAC_SHA256);
        h.insert(&HMAC_SHA512, &_HMAC_SHA512);
        h.insert(&HMAC_SHA1_ETM, &_HMAC_SHA1_ETM);
        h.insert(&HMAC_SHA256_ETM, &_HMAC_SHA256_ETM);
        h.insert(&HMAC_SHA512_ETM, &_HMAC_SHA512_ETM);
        assert_eq!(h.len(), ALL_MAC_ALGORITHMS.len());
        h
    });

#[cfg(test)]
mod md5_compat_tests {
    use super::*;
    use digest::{KeyInit, Mac as _};

    #[test]
    fn rfc_2202_full_length_vectors() {
        let cases: &[(&[u8], &[u8], [u8; 16])] = &[
            (&[0x0b; 16], b"Hi There", [0x92,0x94,0x72,0x7a,0x36,0x38,0xbb,0x1c,0x13,0xf4,0x8e,0xf8,0x15,0x8b,0xfc,0x9d]),
            (b"Jefe", b"what do ya want for nothing?", [0x75,0x0c,0x78,0x3e,0x6a,0xb0,0xb5,0x03,0xea,0xa8,0x6e,0x31,0x0a,0x5d,0xb7,0x38]),
        ];
        for (key, data, expected) in cases {
            let mut mac = <Hmac<Md5> as KeyInit>::new_from_slice(key).unwrap();
            mac.update(data);
            assert_eq!(mac.finalize().into_bytes().as_slice(), expected);
        }
    }

    #[test]
    fn packet_mac_binds_sequence_payload_and_full_tag() {
        let key = [0x37; 16];
        let packet = b"SSH packet fixture";
        let mac = _HMAC_MD5.make_mac(&key);
        assert_eq!(mac.mac_len(), 16);
        assert!(!mac.is_etm());
        let mut tag = [0; 16];
        mac.compute(42, packet, &mut tag);
        let mut reference = <Hmac<Md5> as KeyInit>::new_from_slice(&key).unwrap();
        reference.update(&42_u32.to_be_bytes());
        reference.update(packet);
        assert_eq!(tag.as_slice(), reference.finalize().into_bytes().as_slice());
        assert!(mac.verify(42, packet, &tag));
        assert!(!mac.verify(43, packet, &tag));
        assert!(!mac.verify(42, b"changed packet", &tag));
        let mut changed = tag;
        changed[0] ^= 1;
        assert!(!mac.verify(42, packet, &changed));
        assert!(!mac.verify(42, packet, &tag[..12]));
        assert!(!mac.verify(42, packet, &[]));
    }

    #[test]
    fn md5_registered_but_not_default() {
        assert_eq!(Name::try_from("hmac-md5"), Ok(HMAC_MD5));
        assert!(Name::try_from("hmac-md5-96").is_err());
        assert!(!crate::Preferred::DEFAULT.mac.contains(&HMAC_MD5));
        assert_eq!(_HMAC_MD5.key_len(), 16);
    }
}
