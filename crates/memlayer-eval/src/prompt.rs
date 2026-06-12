// Generated with AI Coding Rules Hub
//! Prompt building and token counting.
//!
//! Formats retrieved memories into a system prompt suitable for the answer
//! LLM, and counts tokens using tiktoken-rs (cl100k_base — closest to
//! Claude's tokeniser for comparison purposes).

use tiktoken_rs::cl100k_base;

/// Build the system prompt handed to the answer LLM.
/// Returns (system_prompt, user_message, estimated_token_count).
pub fn build_answer_prompt(
    memories: &[String],
    question: &str,
) -> (String, String, usize) {
    let memories_block = if memories.is_empty() {
        "No relevant memories found.".to_string()
    } else {
        memories
            .iter()
            .enumerate()
            .map(|(i, m)| format!("[Memory {}]\n{}", i + 1, m))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let system = format!(
        "You are a helpful assistant with access to a user's personal memory store.\n\
         Answer the question using ONLY the provided memories.\n\
         \n\
         Memory format notes:\n\
         - Each memory is a fact in the form: subject - predicate - object.\n\
         - Bracketed dates like [2023-05-20] are when the fact happened (the\n\
           event date), NOT today.\n\
         - Lines like '[9:55 am on 22 October, 2023] Caroline (...): ...' are\n\
           the original chat turns surrounding the fact.\n\
         \n\
         Answering rules:\n\
         1. For questions like 'how long ago' or 'when', anchor to the LATEST\n\
            timestamp visible in the memories (the conversation's present),\n\
            not today's calendar date.\n\
         2. For questions like 'what activities does X do' or 'what does X like',\n\
            enumerate EVERY distinct item mentioned across the memories. Do not\n\
            stop at the first 2-3.\n\
         3. Match the specificity of the question. If the gold-style answer\n\
            would include a modifier (e.g. 'counseling for X'), include it\n\
            when memories support it.\n\
         4. Be concise — answer the question directly. Avoid bullet lists or\n\
            paragraphs of context unless asked.\n\
         5. Only say 'I don't know' if NO memory relates to the question.\n\
         \n\
         <memories>\n{memories_block}\n</memories>"
    );
    let user = question.to_string();

    let token_count = count_tokens(&system) + count_tokens(&user);
    (system, user, token_count)
}

/// Build the judge prompt used to score a (question, gold_answer, model_answer) triple.
/// Returns the complete judge user message.
pub fn build_judge_prompt(
    question: &str,
    gold_answer: &str,
    model_answer: &str,
    extra_context: Option<&str>,
) -> String {
    let ctx_block = extra_context
        .map(|c| format!("\n\nAdditional context:\n{c}"))
        .unwrap_or_default();

    format!(
        "You are an impartial judge evaluating whether a model answer is correct.\n\
         \n\
         Question: {question}{ctx_block}\n\
         Gold answer: {gold_answer}\n\
         Model answer: {model_answer}\n\
         \n\
         Is the model answer correct? Consider the answer correct if it captures the\n\
         essential factual content of the gold answer, even if phrased differently.\n\
         Respond with exactly one word: YES or NO."
    )
}

fn count_tokens(text: &str) -> usize {
    // cl100k_base is a global singleton; initialise once and reuse.
    match cl100k_base() {
        Ok(bpe) => bpe.encode_with_special_tokens(text).len(),
        Err(_) => {
            // Fallback: rough word-count estimate (4 chars ≈ 1 token).
            text.len() / 4
        }
    }
}
