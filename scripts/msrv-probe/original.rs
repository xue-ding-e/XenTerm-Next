// Standalone reproduction of the exact std API used by TriggerEngine.
pub fn boundary(text: &str, index: usize) -> usize {
    text.ceil_char_boundary(index)
}
