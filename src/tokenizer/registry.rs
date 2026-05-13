//! Tokenizer enum + repo-id table. Real implementation lands in Task 2.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tokenizer {
    Cl100k,
    O200k,
    Claude,
    Llama3,
    Qwen3,
}
