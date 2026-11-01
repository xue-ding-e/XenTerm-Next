use super::super::*;

#[test]
fn confirmed_exit_never_reopens_close_prompt() {
    assert!(should_block_close(false, true));
    assert!(!should_block_close(false, false));
    assert!(!should_block_close(true, true));
    assert!(!should_block_close(true, false));
}
