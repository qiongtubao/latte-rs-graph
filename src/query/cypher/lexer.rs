//! Hand-rolled lexer for the Cypher v1 subset.
//!
//! No external parser combinator — every byte is walked by hand. The output
//! is a flat `Vec<Token>` consumed by the recursive-descent parser.
//!
//! Whitespace is skipped. `--` starts a line comment that runs to `\n`.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Match,
    Where,
    Return,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    As,
    Starts,
    With,
    Contains,
    In,
    And,
    Or,
    Not,
    Count,
    Distinct,
    Optional,
    Union,
    All,
    Unwind,
    // Structural
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StrLit(String),
    // Punctuation
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    Colon,
    Dash,
    Star,
    Lt,
    Gt,
    Eq,
    Le,
    Ge,
    Ne,
    Plus,
    Slash,
    DotDot,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Match => f.write_str("MATCH"),
            Token::Where => f.write_str("WHERE"),
            Token::Return => f.write_str("RETURN"),
            Token::Order => f.write_str("ORDER"),
            Token::By => f.write_str("BY"),
            Token::Asc => f.write_str("ASC"),
            Token::Desc => f.write_str("DESC"),
            Token::Limit => f.write_str("LIMIT"),
            Token::As => f.write_str("AS"),
            Token::Starts => f.write_str("STARTS"),
            Token::With => f.write_str("WITH"),
            Token::Contains => f.write_str("CONTAINS"),
            Token::In => f.write_str("IN"),
            Token::And => f.write_str("AND"),
            Token::Or => f.write_str("OR"),
            Token::Not => f.write_str("NOT"),
            Token::Count => f.write_str("COUNT"),
            Token::Distinct => f.write_str("DISTINCT"),
            Token::Optional => f.write_str("OPTIONAL"),
            Token::Union => f.write_str("UNION"),
            Token::All => f.write_str("ALL"),
            Token::Unwind => f.write_str("UNWIND"),
            Token::Ident(s) => write!(f, "ident({s})"),
            Token::IntLit(n) => write!(f, "int({n})"),
            Token::FloatLit(n) => write!(f, "float({n})"),
            Token::StrLit(s) => write!(f, "string({s:?})"),
            Token::LParen => f.write_str("("),
            Token::RParen => f.write_str(")"),
            Token::LBracket => f.write_str("["),
            Token::RBracket => f.write_str("]"),
            Token::LBrace => f.write_str("{"),
            Token::RBrace => f.write_str("}"),
            Token::Comma => f.write_str(","),
            Token::Dot => f.write_str("."),
            Token::Colon => f.write_str(":"),
            Token::Dash => f.write_str("-"),
            Token::Star => f.write_str("*"),
            Token::Lt => f.write_str("<"),
            Token::Gt => f.write_str(">"),
            Token::Eq => f.write_str("="),
            Token::Le => f.write_str("<="),
            Token::Ge => f.write_str(">="),
            Token::Ne => f.write_str("!="),
            Token::Plus => f.write_str("+"),
            Token::Slash => f.write_str("/"),
            Token::DotDot => f.write_str(".."),
        }
    }
}

/// Tokenise the whole input. Whitespace and `-- … \n` comments are skipped.
pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        let b = bytes[i];

        // Skip whitespace.
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Line comment: `--` to end of line.
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Punctuation that might be 2-char.
        if b == b'.' && i + 1 < bytes.len() && bytes[i + 1] == b'.' {
            out.push(Token::DotDot);
            i += 2;
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'=' {
            out.push(Token::Le);
            i += 2;
            continue;
        }
        if b == b'>' && i + 1 < bytes.len() && bytes[i + 1] == b'=' {
            out.push(Token::Ge);
            i += 2;
            continue;
        }
        if b == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'=' {
            out.push(Token::Ne);
            i += 2;
            continue;
        }

        match b {
            b'(' => { out.push(Token::LParen); i += 1; }
            b')' => { out.push(Token::RParen); i += 1; }
            b'[' => { out.push(Token::LBracket); i += 1; }
            b']' => { out.push(Token::RBracket); i += 1; }
            b'{' => { out.push(Token::LBrace); i += 1; }
            b'}' => { out.push(Token::RBrace); i += 1; }
            b',' => { out.push(Token::Comma); i += 1; }
            b':' => { out.push(Token::Colon); i += 1; }
            b'-' => { out.push(Token::Dash); i += 1; }
            b'*' => { out.push(Token::Star); i += 1; }
            b'=' => { out.push(Token::Eq); i += 1; }
            b'<' => { out.push(Token::Lt); i += 1; }
            b'>' => { out.push(Token::Gt); i += 1; }
            b'+' => { out.push(Token::Plus); i += 1; }
            b'/' => { out.push(Token::Slash); i += 1; }
            b'.' => { out.push(Token::Dot); i += 1; }
            b'\'' | b'"' => {
                let quote = b;
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != quote {
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err(format!("unterminated string literal at byte {i}"));
                }
                let raw = std::str::from_utf8(&bytes[start..j])
                    .map_err(|e| format!("non-UTF8 string literal: {e}"))?;
                out.push(Token::StrLit(raw.to_string()));
                i = j + 1;
            }
            d if d.is_ascii_digit() => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'.' && i + 1 < bytes.len()
                    && bytes[i + 1].is_ascii_digit()
                {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let raw = std::str::from_utf8(&bytes[start..i]).unwrap();
                    let v: f64 = raw
                        .parse()
                        .map_err(|e| format!("invalid float {raw:?}: {e}"))?;
                    out.push(Token::FloatLit(v));
                } else {
                    let raw = std::str::from_utf8(&bytes[start..i]).unwrap();
                    let v: i64 = raw
                        .parse()
                        .map_err(|e| format!("invalid integer {raw:?}: {e}"))?;
                    out.push(Token::IntLit(v));
                }
            }
            a if a == b'_' || a.is_ascii_alphabetic() => {
                let start = i;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c == b'_' || c.is_ascii_alphanumeric() {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let word = std::str::from_utf8(&bytes[start..i]).unwrap();
                let tok = match word {
                    "MATCH" => Token::Match,
                    "WHERE" => Token::Where,
                    "RETURN" => Token::Return,
                    "ORDER" => Token::Order,
                    "BY" => Token::By,
                    "ASC" => Token::Asc,
                    "DESC" => Token::Desc,
                    "LIMIT" => Token::Limit,
                    "AS" => Token::As,
                    "STARTS" => Token::Starts,
                    "WITH" => Token::With,
                    "CONTAINS" => Token::Contains,
                    "IN" => Token::In,
                    "AND" => Token::And,
                    "NOT" => Token::Not,
                    "COUNT" => Token::Count,
                    "DISTINCT" => Token::Distinct,
                    "OPTIONAL" => Token::Optional,
                    "UNION" => Token::Union,
                    "ALL" => Token::All,
                    "UNWIND" => Token::Unwind,
                    "true" => Token::StrLit("true".to_string()),
                    "false" => Token::StrLit("false".to_string()),
                    "null" => Token::StrLit("null".to_string()),
                    _ => Token::Ident(word.to_string()),
                };
                out.push(tok);
            }
            other => {
                return Err(format!(
                    "unexpected character {:?} at byte {i}",
                    other as char
                ));
            }
        }
    }

    Ok(out)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected token list for `MATCH (n:Function) RETURN n`. Used as a
    /// snapshot — any drift here means the lexer changed shape.
    #[test]
    fn lex_basic_match() {
        let toks = tokenize("MATCH (n:Function) RETURN n").unwrap();
        let expected = vec![
            Token::Match,
            Token::LParen,
            Token::Ident("n".into()),
            Token::Colon,
            Token::Ident("Function".into()),
            Token::RParen,
            Token::Return,
            Token::Ident("n".into()),
        ];
        assert_eq!(toks, expected);
    }

    /// Both quote styles survive intact so the parser can pick a literal
    /// without caring which delimiter the user chose.
    #[test]
    fn lex_string_quotes() {
        let toks = tokenize("'foo' \"bar\"").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::StrLit("foo".into()),
                Token::StrLit("bar".into()),
            ]
        );
    }

    /// `-- comment` must not leak through; the keyword after it should be
    /// the only surviving token.
    #[test]
    fn lex_ignores_comments() {
        let toks = tokenize("MATCH -- ignore this\n (n) RETURN n").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Match,
                Token::LParen,
                Token::Ident("n".into()),
                Token::RParen,
                Token::Return,
                Token::Ident("n".into()),
            ]
        );
    }
}
