#[path = "impls/certificate_verifier.rs"]
mod certificate_verifier;
#[path = "struct/verifier.rs"]
mod verifier;

pub(crate) use verifier::WebDavAcceptAnyCertVerifier;
