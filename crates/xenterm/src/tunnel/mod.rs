#[path = "impls/forward.rs"]
mod forward;

pub(crate) use forward::*;

#[path = "impls/owner.rs"]
pub(crate) mod owner;
