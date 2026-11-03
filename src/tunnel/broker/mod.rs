//! Experimental cross-process ownership core. Not wired to any production
//! entry point until OS IPC, immutable route snapshots and drain are verified.
//! In particular, absence of a broker adapter must never fall back to local
//! authority on behalf of an HTTP caller.
#![allow(dead_code)]
mod protocol;
mod registry;
mod snapshot;
mod runtime;

mod coordinator;
