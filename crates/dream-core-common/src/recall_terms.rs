//! Breaking a free-text question into the pieces memory is matched against.
//!
//! Shared by both places company memory is recalled — the client's synced copy
//! (`dream_core_system::team_memory`) and the enterprise server's own search —
//! so the same question finds the same items on either side.

/// Worth more than one adjacent-pair hit: a whole word is a far stronger
/// signal than two characters that happen to sit together.
const WORD_WEIGHT: usize = 2;

/// Query pairs of CJK characters that appear in nearly every sentence.
///
/// The same job `is_selective` does for Latin, but a character pair cannot be
/// filtered by length — every pair is two characters — so the weak ones are
/// named. Kept short on purpose: a pair missing from here costs a little
/// precision (the two-hit rule below absorbs most of it), a pair wrongly listed
/// costs recall of a real term.
const WEAK_PAIRS: &[&str] = &[
    "什么", "怎么", "这个", "那个", "一个", "我们", "你们", "他们", "可以", "没有", "不是", "就是", "还是", "或者",
    "以及", "如果", "因为", "所以", "但是", "然后", "现在", "已经", "需要", "请问", "帮我", "一下", "是否", "哪个",
    "哪些", "多少", "如何", "应该", "这样", "那样", "自己", "时候", "是谁", "在哪", "几点",
];

/// Single characters that make a pair a grammar fragment rather than a term:
/// "的话", "了吗", "关的".
const PARTICLES: &[char] = &['的', '了', '吗', '呢', '吧', '啊', '么', '呀', '嘛'];

/// A query broken into the pieces an item is scored against.
pub struct QueryTerms {
    /// Whole words: Latin (≥4 characters) and other word-separating scripts.
    pub words: Vec<String>,
    /// Adjacent character pairs from runs of unsegmented script.
    pub pairs: Vec<String>,
}

impl QueryTerms {
    pub fn from_query(query: &str) -> Self {
        let mut terms = Self {
            words: Vec::new(),
            pairs: Vec::new(),
        };
        let mut run = String::new();
        let mut run_is_cjk = false;

        for c in query.chars() {
            if !c.is_alphanumeric() {
                terms.take_run(&mut run, run_is_cjk);
                continue;
            }
            let cjk = is_unsegmented(c);
            if !run.is_empty() && cjk != run_is_cjk {
                terms.take_run(&mut run, run_is_cjk);
            }
            run_is_cjk = cjk;
            run.push(c);
        }
        terms.take_run(&mut run, run_is_cjk);
        terms
    }

    /// Turn one finished run of same-kind characters into terms.
    fn take_run(&mut self, run: &mut String, is_cjk: bool) {
        if run.is_empty() {
            return;
        }
        if is_cjk {
            let chars: Vec<char> = run.chars().collect();
            for window in chars.windows(2) {
                let pair: String = window.iter().collect();
                let weak = WEAK_PAIRS.contains(&pair.as_str()) || pair.chars().any(|c| PARTICLES.contains(&c));
                if !weak && !self.pairs.contains(&pair) {
                    self.pairs.push(pair);
                }
            }
        } else if is_selective(run) {
            let word = run.to_lowercase();
            if !self.words.contains(&word) {
                self.words.push(word);
            }
        }
        run.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.pairs.is_empty()
    }

    /// How strongly `haystack` (already lowercased) matches.
    pub fn score(&self, haystack: &str) -> usize {
        let words = self.words.iter().filter(|w| haystack.contains(w.as_str())).count();
        let pairs = self.pairs.iter().filter(|p| haystack.contains(p.as_str())).count();
        words * WORD_WEIGHT + pairs
    }

    /// What an item has to reach to count as a match.
    ///
    /// A sentence of n Han characters yields n-1 pairs, so a longer question
    /// offers many chances for one coincidental pair ("项目") to land in an
    /// unrelated item. Once there are enough pairs to afford it, one stray pair
    /// is not enough; a whole word still is.
    pub fn required_score(&self) -> usize {
        if self.pairs.len() >= 4 { 2 } else { 1 }
    }
}

/// Han, kana and Hangul: scripts written without spaces between words.
fn is_unsegmented(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}')
}

/// Whether a whole word carries enough signal to match on.
///
/// Without this, "the" matches almost every sentence ever written and recall
/// fills the prompt with whatever happened to be stored — the first version
/// here surfaced an unrelated item for "what is the weather tomorrow" because
/// one memory began with "The". Injecting the wrong memory is worse than
/// injecting none: the member cannot see why the model suddenly knows about
/// deploy keys.
///
/// A length floor rather than a stopword list, because a stopword list is one
/// per language and this text is routinely mixed. Latin words need four
/// characters, which clears the/is/who/and while keeping anything anyone would
/// actually search for; other scripts that separate words need two.
fn is_selective(word: &str) -> bool {
    let len = word.chars().count();
    if word.is_ascii() { len >= 4 } else { len >= 2 }
}
