// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The code views: a source file, a function's disassembly, and the two
//! interleaved, with syntax highlighting. The model and the lexers; the
//! rows are painted by the app.
//!
//! The lexers are hand-written and linear: a document is scanned once for
//! the state each line starts in (inside a block comment or not), and a
//! visible line is tokenized when it is painted. No regular expressions,
//! no grammar: enough to colour keywords, types, calls, numbers, strings,
//! comments and preprocessor lines the way C++ Orbit's `Cpp` highlighter
//! does, and registers, mnemonics and addresses the way its `X86Assembly`
//! does.

use egui::Color32;

/// What a file is, from its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    C,
    Cpp,
    Asm,
    Plain,
}

impl Language {
    pub fn from_path(path: &str) -> Language {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Language::Rust,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "inl" | "ipp" => Language::Cpp,
            "s" | "asm" => Language::Asm,
            _ => Language::Plain,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Asm => "x86-64",
            Language::Plain => "text",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Keyword,
    Type,
    Function,
    Number,
    Str,
    Comment,
    Preprocessor,
    Punct,
    Macro,
    Attribute,
    /// x86: a register.
    Register,
    /// x86: the mnemonic.
    Mnemonic,
    /// x86: an address or a label.
    Address,
    /// x86: `ptr`, `qword`, sizes.
    Size,
}

/// Darcula, as C++ Orbit's highlighters use it.
pub fn token_color(kind: TokenKind) -> Color32 {
    match kind {
        TokenKind::Plain => Color32::from_rgb(0xBD, 0xBD, 0xBD),
        TokenKind::Keyword => Color32::from_rgb(0xCC, 0x78, 0x32),
        TokenKind::Type => Color32::from_rgb(0xCC, 0xAA, 0xCC),
        TokenKind::Function => Color32::from_rgb(0xFF, 0xCC, 0x66),
        TokenKind::Number => Color32::from_rgb(0x61, 0x96, 0xCC),
        TokenKind::Str => Color32::from_rgb(0x66, 0x99, 0x66),
        TokenKind::Comment => Color32::from_rgb(0x80, 0x80, 0x80),
        TokenKind::Preprocessor => Color32::from_rgb(0xA0, 0xA0, 0x33),
        TokenKind::Punct => Color32::from_rgb(0xA9, 0xB7, 0xC6),
        TokenKind::Macro => Color32::from_rgb(0x99, 0x66, 0x99),
        TokenKind::Attribute => Color32::from_rgb(0xBB, 0xB5, 0x29),
        TokenKind::Register => Color32::from_rgb(0x7E, 0xC6, 0x99),
        TokenKind::Mnemonic => Color32::from_rgb(0xF8, 0xC5, 0x55),
        TokenKind::Address => Color32::from_rgb(0x61, 0x96, 0xCC),
        TokenKind::Size => Color32::from_rgb(0xCC, 0x99, 0xCD),
    }
}

/// A run of one kind: `[start, end)` byte offsets into the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// What a line starts inside of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineState {
    pub in_block_comment: bool,
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "union",
];
const RUST_TYPES: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "bool",
    "char", "str", "String", "Vec", "Option", "Result", "Box", "Arc", "Rc", "HashMap", "HashSet", "Some", "None",
    "Ok", "Err",
];
const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "catch", "class", "const", "constexpr", "consteval", "continue", "default", "delete",
    "do", "else", "enum", "explicit", "export", "extern", "false", "final", "for", "friend", "goto", "if",
    "inline", "mutable", "namespace", "new", "noexcept", "nullptr", "operator", "override", "private",
    "protected", "public", "register", "return", "sizeof", "static", "static_cast", "reinterpret_cast",
    "const_cast", "dynamic_cast", "struct", "switch", "template", "this", "throw", "true", "try", "typedef",
    "typename", "union", "using", "virtual", "volatile", "while", "co_await", "co_return", "co_yield",
    "concept", "requires", "static_assert", "thread_local",
];
const C_TYPES: &[&str] = &[
    "void", "int", "char", "short", "long", "float", "double", "unsigned", "signed", "bool", "size_t", "ssize_t",
    "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "uintptr_t",
    "intptr_t", "pid_t", "std", "string", "vector", "optional", "unique_ptr", "shared_ptr", "absl",
];
const X86_REGISTERS: &[&str] = &[
    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "eax", "ebx", "ecx", "edx", "esi", "edi", "ebp",
    "esp", "ax", "bx", "cx", "dx", "si", "di", "bp", "sp", "al", "bl", "cl", "dl", "ah", "bh", "ch", "dh",
    "sil", "dil", "bpl", "spl", "rip", "eip", "cs", "ds", "es", "fs", "gs", "ss", "eflags", "rflags",
];
const X86_SIZES: &[&str] = &[
    "ptr", "byte", "word", "dword", "qword", "xmmword", "ymmword", "zmmword", "tbyte", "short", "near", "far",
];

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn is_register(word: &str) -> bool {
    if X86_REGISTERS.contains(&word) {
        return true;
    }
    let bytes = word.as_bytes();
    // r8..r15 with b/w/d suffixes, xmm0..31, ymm, zmm, k0..7, st0..7
    let numbered = |prefix: &str| word.starts_with(prefix) && word[prefix.len()..].chars().all(|c| c.is_ascii_digit()) && word.len() > prefix.len();
    if bytes.first() == Some(&b'r') && word.len() >= 2 {
        let rest = word[1..].trim_end_matches(['b', 'w', 'd']);
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    numbered("xmm") || numbered("ymm") || numbered("zmm") || numbered("k") || numbered("st") || numbered("mm")
}

/// Tokenizes one line. Returns the spans and the state the next line
/// starts in. Byte offsets; every byte of the line is covered.
pub fn lex_line(lang: Language, text: &str, state: LineState) -> (Vec<Span>, LineState) {
    match lang {
        Language::Asm => (lex_asm(text), LineState::default()),
        Language::Plain => (vec![Span { start: 0, end: text.len(), kind: TokenKind::Plain }], LineState::default()),
        Language::Rust | Language::C | Language::Cpp => lex_c_like(lang, text, state),
    }
}

fn lex_c_like(lang: Language, text: &str, mut state: LineState) -> (Vec<Span>, LineState) {
    let b = text.as_bytes();
    let n = b.len();
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |start: usize, end: usize, kind: TokenKind| {
        if end > start {
            spans.push(Span { start, end, kind });
        }
    };
    let mut i = 0usize;
    // A preprocessor line (C): the whole line, minus a trailing comment.
    let trimmed = text.trim_start();
    if lang != Language::Rust && trimmed.starts_with('#') && !state.in_block_comment {
        let lead = n - trimmed.len();
        push(0, lead, TokenKind::Plain);
        // `#include <x>` and `"x"` read as strings; the rest is the directive.
        let rest = &text[lead..];
        if let Some(p) = rest.find(['<', '"']) {
            push(lead, lead + p, TokenKind::Preprocessor);
            push(lead + p, n, TokenKind::Str);
        } else if let Some(p) = rest.find("//") {
            push(lead, lead + p, TokenKind::Preprocessor);
            push(lead + p, n, TokenKind::Comment);
        } else {
            push(lead, n, TokenKind::Preprocessor);
        }
        return (spans, state);
    }
    while i < n {
        if state.in_block_comment {
            let start = i;
            match text[i..].find("*/") {
                Some(p) => {
                    i += p + 2;
                    state.in_block_comment = false;
                }
                None => i = n,
            }
            push(start, i, TokenKind::Comment);
            continue;
        }
        let c = b[i];
        // comments
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            push(i, n, TokenKind::Comment);
            i = n;
            continue;
        }
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let start = i;
            i += 2;
            match text[i..].find("*/") {
                Some(p) => i += p + 2,
                None => {
                    i = n;
                    state.in_block_comment = true;
                }
            }
            push(start, i, TokenKind::Comment);
            continue;
        }
        // strings and chars
        if c == b'"' || (c == b'\'' && lang != Language::Rust) {
            let quote = c;
            let start = i;
            i += 1;
            while i < n && b[i] != quote {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            push(start, i, TokenKind::Str);
            continue;
        }
        // Rust: a lifetime or a char literal
        if c == b'\'' && lang == Language::Rust {
            let start = i;
            // 'a' or '\n' is a char; 'a followed by a non-quote is a lifetime.
            let mut j = i + 1;
            if j < n && b[j] == b'\\' {
                j += 2;
            } else {
                j += 1;
            }
            if j < n && b[j] == b'\'' {
                i = j + 1;
                push(start, i, TokenKind::Str);
            } else {
                i += 1;
                while i < n && is_ident(b[i]) {
                    i += 1;
                }
                push(start, i, TokenKind::Keyword);
            }
            continue;
        }
        // Rust attributes and C++ [[attributes]]
        if c == b'#' && lang == Language::Rust && i + 1 < n && (b[i + 1] == b'[' || b[i + 1] == b'!') {
            let start = i;
            match text[i..].find(']') {
                Some(p) => i += p + 1,
                None => i = n,
            }
            push(start, i, TokenKind::Attribute);
            continue;
        }
        // numbers
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            push(start, i, TokenKind::Number);
            continue;
        }
        // identifiers
        if is_ident_start(c) {
            let start = i;
            while i < n && is_ident(b[i]) {
                i += 1;
            }
            let word = &text[start..i];
            let next = b.get(i).copied().unwrap_or(b' ');
            let kind = if lang == Language::Rust {
                if next == b'!' {
                    TokenKind::Macro
                } else if RUST_KEYWORDS.contains(&word) {
                    TokenKind::Keyword
                } else if RUST_TYPES.contains(&word) || word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                    TokenKind::Type
                } else if next == b'(' || (next == b':' && b.get(i + 1) == Some(&b':') && b.get(i + 2) == Some(&b'<')) {
                    TokenKind::Function
                } else {
                    TokenKind::Plain
                }
            } else if C_KEYWORDS.contains(&word) {
                TokenKind::Keyword
            } else if C_TYPES.contains(&word) || word.ends_with("_t") {
                TokenKind::Type
            } else if word.len() > 1 && word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                TokenKind::Macro
            } else if next == b'(' {
                TokenKind::Function
            } else if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            push(start, i, kind);
            continue;
        }
        // punctuation, one run
        let start = i;
        while i < n && !b[i].is_ascii_alphanumeric() && b[i] != b'_' && b[i] != b'"' && b[i] != b'\'' && b[i] != b'/' && b[i] != b'#' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == start {
            // whitespace, or a lone char the rules above declined
            i += 1;
            push(start, i, TokenKind::Plain);
        } else {
            push(start, i, TokenKind::Punct);
        }
    }
    (spans, state)
}

/// x86 as iced prints it: `mov     rbp, rsp`, `call    0x72afd`, with the
/// address column added by the view.
fn lex_asm(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let n = b.len();
    let mut spans = Vec::new();
    let mut i = 0usize;
    let mut seen_mnemonic = false;
    while i < n {
        let c = b[i];
        if c == b';' {
            spans.push(Span { start: i, end: n, kind: TokenKind::Comment });
            break;
        }
        if c.is_ascii_whitespace() {
            let start = i;
            while i < n && b[i].is_ascii_whitespace() {
                i += 1;
            }
            spans.push(Span { start, end: i, kind: TokenKind::Plain });
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < n && is_ident(b[i]) {
                i += 1;
            }
            let word = &text[start..i];
            let kind = if !seen_mnemonic {
                seen_mnemonic = true;
                TokenKind::Mnemonic
            } else if is_register(word) {
                TokenKind::Register
            } else if X86_SIZES.contains(&word) {
                TokenKind::Size
            } else {
                TokenKind::Function
            };
            spans.push(Span { start, end: i, kind });
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let kind = if text[start..i].starts_with("0x") && i - start >= 6 { TokenKind::Address } else { TokenKind::Number };
            spans.push(Span { start, end: i, kind });
            continue;
        }
        let start = i;
        i += 1;
        spans.push(Span { start, end: i, kind: TokenKind::Punct });
    }
    spans
}

/// A source file, split into lines, with the state each line starts in.
#[derive(Clone, Debug, Default)]
pub struct CodeDoc {
    pub path: String,
    pub lang: Language,
    pub lines: Vec<String>,
    /// The state line `i` starts in. One pass at load, so painting any line
    /// is independent of the others.
    pub starts: Vec<LineState>,
}

impl Default for Language {
    fn default() -> Self {
        Language::Plain
    }
}

impl CodeDoc {
    pub fn new(path: &str, text: &str) -> CodeDoc {
        let lang = Language::from_path(path);
        let lines: Vec<String> = text.lines().map(|l| l.replace('\t', "    ")).collect();
        let mut starts = Vec::with_capacity(lines.len());
        let mut state = LineState::default();
        for line in &lines {
            starts.push(state);
            let (_, next) = lex_line(lang, line, state);
            state = next;
        }
        CodeDoc { path: path.to_string(), lang, lines, starts }
    }

    pub fn spans(&self, line: usize) -> Vec<Span> {
        match (self.lines.get(line), self.starts.get(line)) {
            (Some(text), Some(state)) => lex_line(self.lang, text, *state).0,
            _ => Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// One instruction, as the service sends it.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct DisasmLine {
    pub address: u64,
    pub offset: u64,
    #[serde(default)]
    pub bytes: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub new_line: bool,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct DisasmFunction {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub module_path: String,
    pub address: u64,
    pub size: u64,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
}

/// A function's disassembly, as `/api/code/disassembly` and
/// `/api/code/example` send it.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct Disassembly {
    pub function: DisasmFunction,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub lines: Vec<DisasmLine>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub text: String,
}

/// How the view reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeMode {
    Source,
    Disassembly,
    Both,
}

impl CodeMode {
    pub fn label(self) -> &'static str {
        match self {
            CodeMode::Source => "Source",
            CodeMode::Disassembly => "Disassembly",
            CodeMode::Both => "Both",
        }
    }
}

/// One painted row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeRow {
    /// Line `line` (0-based) of the source document.
    Source { line: usize },
    /// Instruction `index` of the disassembly.
    Asm { index: usize },
    /// A line of another file the disassembly refers to (an inlined
    /// callee, mostly): only its file and line, kept out of the way.
    Note { text: String },
}

/// The rows of a view, in order. `Both` is the C++ `AnnotatingLine`
/// layout: the disassembly is the text, and a source line is inserted
/// above the first instruction that belongs to it.
pub fn build_rows(mode: CodeMode, doc: Option<&CodeDoc>, disasm: Option<&Disassembly>) -> Vec<CodeRow> {
    match mode {
        CodeMode::Source => doc.map(|d| (0..d.lines.len()).map(|line| CodeRow::Source { line }).collect()).unwrap_or_default(),
        CodeMode::Disassembly => disasm.map(|d| (0..d.lines.len()).map(|index| CodeRow::Asm { index }).collect()).unwrap_or_default(),
        CodeMode::Both => {
            let Some(d) = disasm else {
                return doc.map(|d| (0..d.lines.len()).map(|line| CodeRow::Source { line }).collect()).unwrap_or_default();
            };
            // Lines of the open file are inserted as source rows. Lines of
            // other files (inlined callees, mostly) would drown the function
            // in notes, so they are painted after the instruction instead;
            // see `other_file_tail`.
            let mut rows = Vec::with_capacity(d.lines.len() * 2);
            let mut last: Option<(&str, u32)> = None;
            for (index, ins) in d.lines.iter().enumerate() {
                if ins.line > 0 && last != Some((ins.file.as_str(), ins.line)) {
                    last = Some((ins.file.as_str(), ins.line));
                    if let Some(doc) = doc {
                        if doc.path == ins.file && (ins.line as usize) <= doc.lines.len() {
                            rows.push(CodeRow::Source { line: ins.line as usize - 1 });
                        }
                    }
                }
                rows.push(CodeRow::Asm { index });
            }
            rows
        }
    }
}

impl CodeDoc {
    /// Where to open a file worth reading: the first line that begins a
    /// function body, past the licence, the imports and the module docs.
    /// The whole file is there above it.
    pub fn first_body_line(&self) -> usize {
        self.lines
            .iter()
            .position(|l| {
                let t = l.trim_end();
                let head = t.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
                head && !t.starts_with("use ") && !t.starts_with("namespace") && t.contains('(') && t.ends_with('{')
            })
            .unwrap_or(0)
    }
}

/// What to paint after an instruction that begins a line of a file other
/// than the one open: `file:line`, when the line table says so.
pub fn other_file_tail(ins: &DisasmLine, doc: Option<&CodeDoc>) -> Option<String> {
    if !ins.new_line || ins.line == 0 || ins.file.is_empty() {
        return None;
    }
    if doc.is_some_and(|d| d.path == ins.file) {
        return None;
    }
    Some(format!("{}:{}", ins.file.rsplit('/').next().unwrap_or(&ins.file), ins.line))
}

/// Example code to look at with nothing captured: two files of this
/// repository, embedded at build time so they open anywhere.
pub const EXAMPLE_RUST_PATH: &str = "rust/crates/orbit-service/src/uprobes.rs";
pub const EXAMPLE_RUST: &str = include_str!("../../../../../rust/crates/orbit-service/src/uprobes.rs");
pub const EXAMPLE_CPP_PATH: &str = "src/LinuxTracing/UprobesUnwindingVisitor.cpp";
pub const EXAMPLE_CPP: &str = include_str!("../../../../../src/LinuxTracing/UprobesUnwindingVisitor.cpp");

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Language, text: &str) -> Vec<(String, TokenKind)> {
        let (spans, _) = lex_line(lang, text, LineState::default());
        spans.iter().filter(|s| !text[s.start..s.end].trim().is_empty()).map(|s| (text[s.start..s.end].to_string(), s.kind)).collect()
    }

    #[test]
    fn rust_keywords_types_calls_macros_and_strings() {
        let k = kinds(Language::Rust, r#"pub fn poll(&mut self) -> Vec<CompletedCall> { println!("hi {}", x); // note"#);
        assert!(k.contains(&("pub".into(), TokenKind::Keyword)));
        assert!(k.contains(&("fn".into(), TokenKind::Keyword)));
        assert!(k.contains(&("poll".into(), TokenKind::Function)));
        assert!(k.contains(&("Vec".into(), TokenKind::Type)));
        assert!(k.contains(&("CompletedCall".into(), TokenKind::Type)));
        assert!(k.contains(&("println".into(), TokenKind::Macro)));
        assert!(k.contains(&("\"hi {}\"".into(), TokenKind::Str)));
        assert!(k.iter().any(|(t, kind)| t.starts_with("// note") && *kind == TokenKind::Comment));
    }

    #[test]
    fn cpp_preprocessor_and_block_comments_carry_state() {
        let (spans, state) = lex_line(Language::Cpp, "#include <vector>", LineState::default());
        assert_eq!(spans[0].kind, TokenKind::Preprocessor);
        assert_eq!(spans.last().unwrap().kind, TokenKind::Str);
        assert!(!state.in_block_comment);
        let (spans, state) = lex_line(Language::Cpp, "int x = 1; /* open", LineState::default());
        assert!(state.in_block_comment);
        assert_eq!(spans.last().unwrap().kind, TokenKind::Comment);
        let (spans, state) = lex_line(Language::Cpp, "still */ return x;", state);
        assert_eq!(spans[0].kind, TokenKind::Comment);
        assert!(!state.in_block_comment);
        assert!(spans.iter().any(|s| s.kind == TokenKind::Keyword));
    }

    #[test]
    fn asm_mnemonic_registers_addresses() {
        let k = kinds(Language::Asm, "mov     qword ptr [rbp-0x2b8], rax");
        assert_eq!(k[0], ("mov".into(), TokenKind::Mnemonic));
        assert!(k.contains(&("qword".into(), TokenKind::Size)));
        assert!(k.contains(&("rbp".into(), TokenKind::Register)));
        assert!(k.contains(&("rax".into(), TokenKind::Register)));
        assert!(k.contains(&("0x2b8".into(), TokenKind::Number)));
        let k = kinds(Language::Asm, "call    0x72afd");
        assert!(k.contains(&("0x72afd".into(), TokenKind::Address)));
        assert!(is_register("r15") && is_register("r8d") && is_register("xmm12") && !is_register("rust"));
    }

    #[test]
    fn a_document_scans_its_states_once_and_lexes_lines_alone() {
        let doc = CodeDoc::new("a.rs", "/* a\nb */ fn x() {}\nlet y = 1;");
        assert_eq!(doc.lines.len(), 3);
        assert!(!doc.starts[0].in_block_comment && doc.starts[1].in_block_comment && !doc.starts[2].in_block_comment);
        let spans = doc.spans(1);
        assert_eq!(spans[0].kind, TokenKind::Comment);
        assert!(spans.iter().any(|s| s.kind == TokenKind::Keyword));
    }

    #[test]
    fn both_puts_a_source_line_above_its_first_instruction() {
        let doc = CodeDoc::new("/x/a.c", "int a;\nint b;\nint c;");
        let mut d = Disassembly::default();
        let ins = |address: u64, line: u32, file: &str| DisasmLine { address, line, file: file.into(), ..Default::default() };
        d.lines = vec![ins(0, 2, "/x/a.c"), ins(1, 2, "/x/a.c"), ins(2, 3, "/x/a.c"), ins(3, 7, "/y/b.h"), ins(4, 0, "")];
        let rows = build_rows(CodeMode::Both, Some(&doc), Some(&d));
        assert_eq!(
            rows,
            vec![
                CodeRow::Source { line: 1 },
                CodeRow::Asm { index: 0 },
                CodeRow::Asm { index: 1 },
                CodeRow::Source { line: 2 },
                CodeRow::Asm { index: 2 },
                CodeRow::Asm { index: 3 },
                CodeRow::Asm { index: 4 },
            ]
        );
        let mut other = d.lines[3].clone();
        other.new_line = true;
        assert_eq!(other_file_tail(&other, Some(&doc)).as_deref(), Some("b.h:7"));
        assert_eq!(other_file_tail(&d.lines[3], Some(&doc)), None);
        let mut own = d.lines[0].clone();
        own.new_line = true;
        assert_eq!(other_file_tail(&own, Some(&doc)), None);
        assert_eq!(other_file_tail(&own, None).as_deref(), Some("a.c:2"));
        assert_eq!(build_rows(CodeMode::Source, Some(&doc), None).len(), 3);
        assert_eq!(build_rows(CodeMode::Disassembly, None, Some(&d)).len(), 5);
        assert_eq!(build_rows(CodeMode::Both, None, Some(&d)).len(), 5);
        let examples = CodeDoc::new(EXAMPLE_RUST_PATH, EXAMPLE_RUST);
        assert!(examples.lines.len() > 100 && examples.lang == Language::Rust);
        assert!(examples.first_body_line() > 20, "{}", examples.first_body_line());
        assert!(CodeDoc::new(EXAMPLE_CPP_PATH, EXAMPLE_CPP).first_body_line() > 40);
        assert_eq!(CodeDoc::new(EXAMPLE_CPP_PATH, EXAMPLE_CPP).lang, Language::Cpp);
    }
}
