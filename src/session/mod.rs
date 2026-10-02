#[path = "impls/prompt_queue.rs"]
mod prompt_queue;
#[path = "protocol.rs"]
pub(crate) mod protocol;
#[path = "struct/prompts.rs"]
mod prompts;
#[path = "impls/session.rs"]
mod session;

pub(crate) use prompts::ConnectCtx;
// The queueing policy over those types: which prompt is shown, which merges into
// one already on screen, and what an answer means. Shared by both frontends, because
// two answers to "has this host been confirmed" would be two security policies.
// `PromptOutcome` stays inside: callers ask it `should_show()` rather than matching,
// so the enum is an implementation detail of the answer.
pub(crate) use prompt_queue::{
    abort_window, credential_prompt, enqueue_credential, enqueue_host_key, enqueue_mfa,
    forget_credentials, host_key_prompt, mfa_prompt, resolve_credential, resolve_host_key,
    resolve_mfa,
};
