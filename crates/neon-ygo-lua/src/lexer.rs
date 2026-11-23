use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LexError {
    #[error("unexpected character '{0}' at line {1}")]
    UnexpectedChar(char, usize),
    #[error("unterminated string at line {0}")]
    UnterminatedString(usize),
    #[error("invalid number '{0}' at line {1}")]
    InvalidNumber(String, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    And, Break, Do, Else, ElseIf, End, False, For, Function, Goto,
    If, In, Local, Nil, Not, Or, Repeat, Return, Then, True, Until, While,

    // Literals & Identifiers
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),

    // Operators & Delimiters
    Plus, Minus, Star, Slash, SlashSlash, Percent, Caret, Hash,
    Amp, Tilde, Pipe, LtLt, GtGt,
    EqEq, TildeEq, LtEq, GtEq, Lt, Gt, Eq,
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    ColonColon, Semi, Colon, Comma, Dot, DotDot, DotDotDot,

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    let chars: Vec<char> = src.chars().collect();
    let len = chars.len();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut tokens = Vec::new();

    while i < len {
        let ch = chars[i];
        // Whitespace
        if ch == ' ' || ch == '\t' || ch == '\x0c' {
            i += 1;
            continue;
        }
        if ch == '\n' || ch == '\r' {
            line += 1;
            if ch == '\r' && i + 1 < len && chars[i + 1] == '\n' {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        // Comments: `--` or `--[[...]]`
        if ch == '-' && i + 1 < len && chars[i + 1] == '-' {
            i += 2;
            if i < len && chars[i] == '[' {
                if let Some((end_idx, nl)) = read_long_bracket(&chars, i) {
                    line += nl;
                    i = end_idx;
                    continue;
                }
            }
            while i < len && chars[i] != '\n' && chars[i] != '\r' {
                i += 1;
            }
            continue;
        }

        // Long string `[[...]]` or `[=[...]=]`
        if ch == '[' && i + 1 < len && (chars[i + 1] == '[' || chars[i + 1] == '=') {
            if let Some((s, end_idx, nl)) = read_long_string(&chars, i) {
                tokens.push(Token { kind: TokenKind::Str(s), line });
                line += nl;
                i = end_idx;
                continue;
            }
        }

        // Quoted string
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start_line = line;
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < len {
                let c = chars[i];
                if c == quote {
                    i += 1;
                    closed = true;
                    break;
                }
                if c == '\n' {
                    return Err(LexError::UnterminatedString(start_line));
                }
                if c == '\\' && i + 1 < len {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        '\'' => s.push('\''),
                        '0' => s.push('\0'),
                        '\n' => {
                            line += 1;
                            s.push('\n');
                        }
                        other => s.push(other),
                    }
                    i += 1;
                } else {
                    s.push(c);
                    i += 1;
                }
            }
            if !closed {
                return Err(LexError::UnterminatedString(start_line));
            }
            tokens.push(Token { kind: TokenKind::Str(s), line: start_line });
            continue;
        }

        // Numbers (hex `0x...`, decimal int/float)
        if ch.is_ascii_digit() || (ch == '.' && i + 1 < len && chars[i + 1].is_ascii_digit()) {
            let start = i;
            if ch == '0' && i + 1 < len && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                i += 2;
                while i < len && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex_str: String = chars[start + 2..i].iter().collect();
                let val = u64::from_str_radix(&hex_str, 16)
                    .map_err(|_| LexError::InvalidNumber(format!("0x{hex_str}"), line))?;
                tokens.push(Token { kind: TokenKind::Int(val as i64), line });
                continue;
            }
            let mut is_float = false;
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i < len && chars[i] == '.' && !(i + 1 < len && chars[i + 1] == '.') {
                is_float = true;
                i += 1;
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < len && (chars[i] == 'e' || chars[i] == 'E') {
                is_float = true;
                i += 1;
                if i < len && (chars[i] == '+' || chars[i] == '-') {
                    i += 1;
                }
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let num_str: String = chars[start..i].iter().collect();
            if is_float {
                let v: f64 = num_str.parse().map_err(|_| LexError::InvalidNumber(num_str, line))?;
                tokens.push(Token { kind: TokenKind::Float(v), line });
            } else {
                let v: i64 = num_str
                    .parse()
                    .or_else(|_| num_str.parse::<u64>().map(|u| u as i64))
                    .map_err(|_| LexError::InvalidNumber(num_str, line))?;
                tokens.push(Token { kind: TokenKind::Int(v), line });
            }
            continue;
        }

        // Identifiers & Keywords
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            let kind = match ident.as_str() {
                "and" => TokenKind::And,
                "break" => TokenKind::Break,
                "do" => TokenKind::Do,
                "else" => TokenKind::Else,
                "elseif" => TokenKind::ElseIf,
                "end" => TokenKind::End,
                "false" => TokenKind::False,
                "for" => TokenKind::For,
                "function" => TokenKind::Function,
                "goto" => TokenKind::Goto,
                "if" => TokenKind::If,
                "in" => TokenKind::In,
                "local" => TokenKind::Local,
                "nil" => TokenKind::Nil,
                "not" => TokenKind::Not,
                "or" => TokenKind::Or,
                "repeat" => TokenKind::Repeat,
                "return" => TokenKind::Return,
                "then" => TokenKind::Then,
                "true" => TokenKind::True,
                "until" => TokenKind::Until,
                "while" => TokenKind::While,
                _ => TokenKind::Ident(ident),
            };
            tokens.push(Token { kind, line });
            continue;
        }

        // Multi-character & single-character operators
        let next = if i + 1 < len { Some(chars[i + 1]) } else { None };
        let next2 = if i + 2 < len { Some(chars[i + 2]) } else { None };

        let (kind, adv) = match (ch, next, next2) {
            ('.', Some('.'), Some('.')) => (TokenKind::DotDotDot, 3),
            ('.', Some('.'), _) => (TokenKind::DotDot, 2),
            ('/', Some('/'), _) => (TokenKind::SlashSlash, 2),
            ('<', Some('<'), _) => (TokenKind::LtLt, 2),
            ('>', Some('>'), _) => (TokenKind::GtGt, 2),
            ('=', Some('='), _) => (TokenKind::EqEq, 2),
            ('~', Some('='), _) => (TokenKind::TildeEq, 2),
            ('<', Some('='), _) => (TokenKind::LtEq, 2),
            ('>', Some('='), _) => (TokenKind::GtEq, 2),
            (':', Some(':'), _) => (TokenKind::ColonColon, 2),
            ('+', _, _) => (TokenKind::Plus, 1),
            ('-', _, _) => (TokenKind::Minus, 1),
            ('*', _, _) => (TokenKind::Star, 1),
            ('/', _, _) => (TokenKind::Slash, 1),
            ('%', _, _) => (TokenKind::Percent, 1),
            ('^', _, _) => (TokenKind::Caret, 1),
            ('#', _, _) => (TokenKind::Hash, 1),
            ('&', _, _) => (TokenKind::Amp, 1),
            ('~', _, _) => (TokenKind::Tilde, 1),
            ('|', _, _) => (TokenKind::Pipe, 1),
            ('<', _, _) => (TokenKind::Lt, 1),
            ('>', _, _) => (TokenKind::Gt, 1),
            ('=', _, _) => (TokenKind::Eq, 1),
            ('(', _, _) => (TokenKind::LParen, 1),
            (')', _, _) => (TokenKind::RParen, 1),
            ('{', _, _) => (TokenKind::LBrace, 1),
            ('}', _, _) => (TokenKind::RBrace, 1),
            ('[', _, _) => (TokenKind::LBracket, 1),
            (']', _, _) => (TokenKind::RBracket, 1),
            (';', _, _) => (TokenKind::Semi, 1),
            (':', _, _) => (TokenKind::Colon, 1),
            (',', _, _) => (TokenKind::Comma, 1),
            ('.', _, _) => (TokenKind::Dot, 1),
            _ => return Err(LexError::UnexpectedChar(ch, line)),
        };
        tokens.push(Token { kind, line });
        i += adv;
    }

    tokens.push(Token { kind: TokenKind::Eof, line });
    Ok(tokens)
}

fn read_long_bracket(chars: &[char], start: usize) -> Option<(usize, usize)> {
    read_long_string(chars, start).map(|(_, end, nl)| (end, nl))
}

fn read_long_string(chars: &[char], start: usize) -> Option<(String, usize, usize)> {
    let len = chars.len();
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut eq_count = 0usize;
    let mut i = start + 1;
    while i < len && chars[i] == '=' {
        eq_count += 1;
        i += 1;
    }
    if i >= len || chars[i] != '[' {
        return None;
    }
    i += 1;
    let mut newlines = 0usize;
    if i < len && chars[i] == '\n' {
        newlines += 1;
        i += 1;
    }
    let mut out = String::new();
    while i < len {
        if chars[i] == ']' {
            let mut k = i + 1;
            let mut matched = 0usize;
            while k < len && chars[k] == '=' && matched < eq_count {
                matched += 1;
                k += 1;
            }
            if matched == eq_count && k < len && chars[k] == ']' {
                return Some((out, k + 1, newlines));
            }
        }
        if chars[i] == '\n' {
            newlines += 1;
        }
        out.push(chars[i]);
        i += 1;
    }
    None
}
