//! Recursive-descent parser for the Cypher v1 subset.
//!
//! Grammar:
//! ```text
//! query     := MATCH pattern [WHERE expr] RETURN return_clause
//!              [ORDER BY order_item [ASC|DESC]] [LIMIT int]
//! pattern   := single_node | directed_edge | variable_length
//! single_node    := '(' ident [':' label] ')'
//! directed_edge  := single_node '-[' [':' edge_type] ']->' single_node
//! variable_length:= single_node '-[' [':' edge_type] '*' int '..' int ']->' single_node
//! return_clause := DISTINCT? return_item (',' return_item)*
//! return_item   := (COUNT '(' ('*' | ident) ')' | (ident | ident '.' ident)) [AS ident]
//! expr          := primary
//! primary       := literal | ident '.' ident
//!                | ident '.' ident STARTS WITH literal
//!                | ident '.' ident CONTAINS literal
//!                | ident '.' ident IN '[' literal (',' literal)* ']'
//! literal       := string | int | float | 'true' | 'false' | 'null'
//! ```
//!
//! Anything not in the subset is rejected with a precise message rather
//! than silently skipped (e.g. `OPTIONAL MATCH`, `UNION`, `UNWIND`,
//! `WITH`, multiple `MATCH`, reverse arrows).

use crate::query::cypher::ast::{
    BinOp, CypherQuery, EdgePattern, Expr, Literal, MatchClause, NodePattern, OrderBy, Pattern,
    ReturnClause, ReturnExpr, ReturnItem,
};
use crate::query::cypher::lexer::{tokenize, Token};

/// Parse a full Cypher query (v1 subset). Returns the AST or a
/// human-readable error suitable for `GraphError::Config`.
pub fn parse_cypher(input: &str) -> Result<CypherQuery, String> {
    let toks = tokenize(input)?;
    let mut p = Parser { toks, pos: 0 };
    let q = p.parse_query()?;
    if p.pos < p.toks.len() {
        return Err(format!(
            "unexpected trailing token {} at position {}",
            p.toks[p.pos], p.pos
        ));
    }
    Ok(q)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, want: &Token) -> Result<(), String> {
        match self.eat() {
            Some(t) if &t == want => Ok(()),
            Some(t) => Err(format!(
                "expected {} but found {} at token {}",
                want, t, self.pos
            )),
            None => Err(format!("expected {} but reached end of input", want)),
        }
    }

    fn expect_keyword(&mut self, kw: &Token) -> Result<(), String> {
        self.expect(kw)
    }

    fn parse_query(&mut self) -> Result<CypherQuery, String> {
        // Reject OPTIONAL MATCH up front — that's the only leading
        // modifier we expect on a MATCH in our subset.
        if matches!(self.peek(), Some(Token::Optional)) {
            return Err(
                "OPTIONAL MATCH not supported in v1 — drop the OPTIONAL keyword".to_string(),
            );
        }
        // Reject UNION / UNWIND / WITH at the top level too — they
        // would otherwise masquerade as keyword starts.
        match self.peek() {
            Some(Token::Union) => {
                return Err("UNION not supported in v1".to_string())
            }
            Some(Token::Unwind) => {
                return Err("UNWIND not supported in v1".to_string())
            }
            Some(Token::With) => {
                return Err("WITH not supported in v1".to_string())
            }
            _ => {}
        }
        // 1. MATCH
        self.expect_keyword(&Token::Match)?;
        let pattern = self.parse_pattern()?;
        if matches!(self.peek(), Some(Token::Match)) {
            return Err("multiple MATCH clauses not supported in v1".to_string());
        }

        // 2. Optional WHERE
        let where_clause = if matches!(self.peek(), Some(Token::Where)) {
            self.eat();
            Some(self.parse_expr()?)
        } else {
            None
        };

        // 3. RETURN
        if !matches!(self.peek(), Some(Token::Return)) {
            return Err(format!(
                "expected RETURN but found {:?}",
                self.peek()
            ));
        }
        self.eat();
        let return_clause = self.parse_return_clause()?;

        // 4. Optional ORDER BY
        let order_by = if matches!(self.peek(), Some(Token::Order)) {
            self.eat();
            self.expect_keyword(&Token::By)?;
            let ob = self.parse_order_item()?;
            // optional ASC / DESC
            let descending = match self.peek() {
                Some(Token::Asc) => {
                    self.eat();
                    false
                }
                Some(Token::Desc) => {
                    self.eat();
                    true
                }
                _ => false,
            };
            Some(OrderBy {
                var: ob.0,
                prop: ob.1,
                descending,
            })
        } else {
            None
        };

        // 5. Optional LIMIT
        let limit = if matches!(self.peek(), Some(Token::Limit)) {
            self.eat();
            match self.eat() {
                Some(Token::IntLit(n)) if n > 0 => Some(n as u32),
                Some(Token::IntLit(n)) => {
                    return Err(format!("LIMIT must be positive, got {n}"))
                }
                other => {
                    return Err(format!("expected positive integer after LIMIT, found {:?}", other))
                }
            }
        } else {
            None
        };

        Ok(CypherQuery {
            match_clause: MatchClause { pattern },
            where_clause,
            return_clause,
            order_by,
            limit,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let from = self.parse_node_pattern()?;
        // No arrow => single-node pattern.
        if !matches!(self.peek(), Some(Token::Dash)) {
            return Ok(Pattern::SingleNode(from));
        }

        // Directed edge or variable-length.
        self.eat(); // -
        self.expect(&Token::LBracket)?;

        let var = if matches!(self.peek(), Some(Token::Ident(_))) {
            match self.eat() {
                Some(Token::Ident(s)) => Some(s),
                _ => unreachable!(),
            }
        } else {
            None
        };
        let kind = if matches!(self.peek(), Some(Token::Colon)) {
            self.eat();
            match self.eat() {
                Some(Token::Ident(s)) => Some(s.to_ascii_uppercase()),
                Some(t) => return Err(format!("expected edge kind identifier, found {t}")),
                None => return Err("expected edge kind identifier, found EOF".to_string()),
            }
        } else {
            None
        };

        // Variable-length? `*N..M`
        let var_len = if matches!(self.peek(), Some(Token::Star)) {
            self.eat();
            let min = self.parse_positive_int("min hops")?;
            self.expect(&Token::DotDot)?;
            let max = self.parse_positive_int("max hops")?;
            if min > max {
                return Err(format!(
                    "invalid variable-length range *{min}..{max}: min > max"
                ));
            }
            Some((min, max))
        } else {
            None
        };

        self.expect(&Token::RBracket)?;
        // `-[]->` is three tokens in the lexer: Dash Gt. Consume both.
        self.expect(&Token::Dash)?;
        self.expect(&Token::Gt)?;

        let to = self.parse_node_pattern()?;

        let edge = EdgePattern { var, kind };

        match var_len {
            None => Ok(Pattern::DirectedEdge { from, edge, to }),
            Some((min_hops, max_hops)) => Ok(Pattern::VariableLength {
                from,
                edge,
                min_hops,
                max_hops,
                to,
            }),
        }
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern, String> {
        self.expect(&Token::LParen)?;
        let var = match self.eat() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(format!("expected node variable identifier, found {t}")),
            None => return Err("expected node variable identifier, found EOF".to_string()),
        };
        let label = if matches!(self.peek(), Some(Token::Colon)) {
            self.eat();
            match self.eat() {
                Some(Token::Ident(s)) => Some(s),
                Some(t) => return Err(format!("expected label identifier, found {t}")),
                None => return Err("expected label identifier, found EOF".to_string()),
            }
        } else {
            None
        };
        self.expect(&Token::RParen)?;
        Ok(NodePattern { var, label })
    }

    fn parse_positive_int(&mut self, what: &str) -> Result<u32, String> {
        match self.eat() {
            Some(Token::IntLit(n)) if n >= 0 => Ok(n as u32),
            Some(Token::IntLit(n)) => Err(format!("{what} must be non-negative, got {n}")),
            Some(t) => Err(format!("expected {what} integer, found {t}")),
            None => Err(format!("expected {what} integer, found EOF")),
        }
    }

    fn parse_return_clause(&mut self) -> Result<ReturnClause, String> {
        let distinct = if matches!(self.peek(), Some(Token::Distinct)) {
            self.eat();
            true
        } else {
            false
        };
        let mut items = vec![self.parse_return_item()?];
        while matches!(self.peek(), Some(Token::Comma)) {
            self.eat();
            items.push(self.parse_return_item()?);
        }
        Ok(ReturnClause { items, distinct })
    }

    fn parse_return_item(&mut self) -> Result<ReturnItem, String> {
        // COUNT('(' ('*' | ident) ')') [AS ident]
        if matches!(self.peek(), Some(Token::Count)) {
            self.eat();
            self.expect(&Token::LParen)?;
            let expr = match self.eat() {
                Some(Token::Star) => ReturnExpr::CountStar,
                Some(Token::Ident(s)) => ReturnExpr::CountVar(s),
                Some(t) => return Err(format!("expected '*' or variable inside COUNT, found {t}")),
                None => return Err("expected COUNT argument, found EOF".to_string()),
            };
            self.expect(&Token::RParen)?;
            let alias = self.parse_alias()?;
            return Ok(ReturnItem { expr, alias });
        }

        // Otherwise: ident ('.' ident)?
        let var = match self.eat() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(format!("expected variable identifier in RETURN, found {t}")),
            None => return Err("expected variable identifier in RETURN, found EOF".to_string()),
        };
        let expr = if matches!(self.peek(), Some(Token::Dot)) {
            self.eat();
            match self.eat() {
                Some(Token::Ident(p)) => ReturnExpr::Property { var, prop: p },
                Some(t) => return Err(format!("expected property name after '.', found {t}")),
                None => return Err("expected property name after '.', found EOF".to_string()),
            }
        } else {
            ReturnExpr::Var(var)
        };
        let alias = self.parse_alias()?;
        Ok(ReturnItem { expr, alias })
    }

    fn parse_alias(&mut self) -> Result<Option<String>, String> {
        if matches!(self.peek(), Some(Token::As)) {
            self.eat();
            match self.eat() {
                Some(Token::Ident(s)) => Ok(Some(s)),
                Some(t) => Err(format!("expected alias identifier after AS, found {t}")),
                None => Err("expected alias identifier after AS, found EOF".to_string()),
            }
        } else {
            Ok(None)
        }
    }

    fn parse_order_item(&mut self) -> Result<(String, Option<String>), String> {
        let var = match self.eat() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(format!("expected identifier in ORDER BY, found {t}")),
            None => return Err("expected identifier in ORDER BY, found EOF".to_string()),
        };
        let prop = if matches!(self.peek(), Some(Token::Dot)) {
            self.eat();
            match self.eat() {
                Some(Token::Ident(p)) => Some(p),
                Some(t) => return Err(format!("expected property name after '.', found {t}")),
                None => return Err("expected property name after '.', found EOF".to_string()),
            }
        } else {
            None
        };
        Ok((var, prop))
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        // var.prop STARTS WITH literal | var.prop CONTAINS literal
        // var.prop IN [literal, ...]
        // var.prop <op> literal
        // literal
        let (var, prop) = match self.eat() {
            Some(Token::Ident(s)) => {
                if matches!(self.peek(), Some(Token::Dot)) {
                    self.eat();
                    let p = match self.eat() {
                        Some(Token::Ident(p)) => p,
                        Some(t) => {
                            return Err(format!(
                                "expected property name after '.', found {t}"
                            ))
                        }
                        None => {
                            return Err(
                                "expected property name after '.', found EOF".to_string()
                            )
                        }
                    };
                    (s, p)
                } else {
                    return Err(format!(
                        "expected property expression in WHERE, found bare variable {s}"
                    ));
                }
            }
            Some(Token::StrLit(_)) | Some(Token::IntLit(_)) | Some(Token::FloatLit(_)) => {
                return Err(
                    "WHERE clause must be a property expression (var.prop …)".to_string(),
                )
            }
            Some(t) => {
                return Err(format!(
                    "expected property expression in WHERE, found {t}"
                ))
            }
            None => return Err("expected WHERE expression, found EOF".to_string()),
        };

        // STARTS WITH / CONTAINS / IN / op
        match self.peek() {
            Some(Token::Starts) => {
                self.eat();
                self.expect(&Token::With)?;
                let value = self.parse_literal()?;
                Ok(Expr::StartsWith { var, prop, value })
            }
            Some(Token::Contains) => {
                self.eat();
                let value = self.parse_literal()?;
                Ok(Expr::Contains { var, prop, value })
            }
            Some(Token::In) => {
                self.eat();
                self.expect(&Token::LBracket)?;
                let mut values = Vec::new();
                if !matches!(self.peek(), Some(Token::RBracket)) {
                    values.push(self.parse_literal()?);
                    while matches!(self.peek(), Some(Token::Comma)) {
                        self.eat();
                        values.push(self.parse_literal()?);
                    }
                }
                self.expect(&Token::RBracket)?;
                Ok(Expr::InList { var, prop, values })
            }
            Some(Token::Eq) => {
                self.eat();
                let right = self.parse_literal()?;
                Ok(Expr::BinaryOp {
                    op: BinOp::Eq,
                    left: Box::new(Expr::Property { var, prop }),
                    right: Box::new(Expr::Literal(right)),
                })
            }
            Some(Token::Ne) => {
                self.eat();
                let right = self.parse_literal()?;
                Ok(Expr::BinaryOp {
                    op: BinOp::Ne,
                    left: Box::new(Expr::Property { var, prop }),
                    right: Box::new(Expr::Literal(right)),
                })
            }
            Some(Token::Lt) => {
                self.eat();
                let right = self.parse_literal()?;
                Ok(Expr::BinaryOp {
                    op: BinOp::Lt,
                    left: Box::new(Expr::Property { var, prop }),
                    right: Box::new(Expr::Literal(right)),
                })
            }
            Some(Token::Le) => {
                self.eat();
                let right = self.parse_literal()?;
                Ok(Expr::BinaryOp {
                    op: BinOp::Le,
                    left: Box::new(Expr::Property { var, prop }),
                    right: Box::new(Expr::Literal(right)),
                })
            }
            Some(Token::Gt) => {
                self.eat();
                let right = self.parse_literal()?;
                Ok(Expr::BinaryOp {
                    op: BinOp::Gt,
                    left: Box::new(Expr::Property { var, prop }),
                    right: Box::new(Expr::Literal(right)),
                })
            }
            Some(Token::Ge) => {
                self.eat();
                let right = self.parse_literal()?;
                Ok(Expr::BinaryOp {
                    op: BinOp::Ge,
                    left: Box::new(Expr::Property { var, prop }),
                    right: Box::new(Expr::Literal(right)),
                })
            }
            Some(t) => Err(format!(
                "expected comparison/STARTS WITH/CONTAINS/IN in WHERE, found {t}"
            )),
            None => Err("expected comparison/STARTS WITH/CONTAINS/IN in WHERE, found EOF".to_string()),
        }
    }

    fn parse_literal(&mut self) -> Result<Literal, String> {
        match self.eat() {
            Some(Token::StrLit(s)) => match s.as_str() {
                "true" => Ok(Literal::Bool(true)),
                "false" => Ok(Literal::Bool(false)),
                "null" => Ok(Literal::Null),
                _ => Ok(Literal::Str(s)),
            },
            Some(Token::IntLit(n)) => Ok(Literal::Int(n)),
            Some(Token::FloatLit(n)) => Ok(Literal::Float(n)),
            Some(t) => Err(format!("expected literal, found {t}")),
            None => Err("expected literal, found EOF".to_string()),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_match() {
        let q = parse_cypher("MATCH (n:Function) RETURN n").unwrap();
        assert!(matches!(q.match_clause.pattern, Pattern::SingleNode(_)));
        assert_eq!(q.return_clause.items.len(), 1);
        assert!(matches!(
            q.return_clause.items[0].expr,
            ReturnExpr::Var(_)
        ));
        assert!(q.where_clause.is_none());
        assert!(q.limit.is_none());
    }

    #[test]
    fn parse_relationship() {
        let q =
            parse_cypher("MATCH (n:Function)-[:CALLS]->(m) RETURN n, m").unwrap();
        match q.match_clause.pattern {
            Pattern::DirectedEdge { from, edge, to } => {
                assert_eq!(from.var, "n");
                assert_eq!(from.label.as_deref(), Some("Function"));
                assert_eq!(edge.kind.as_deref(), Some("CALLS"));
                assert_eq!(to.var, "m");
            }
            other => panic!("expected DirectedEdge, got {other:?}"),
        }
        assert_eq!(q.return_clause.items.len(), 2);
    }

    #[test]
    fn parse_variable_length() {
        let q = parse_cypher("MATCH (n:Function)-[:CALLS*1..3]->(m) RETURN m")
            .unwrap();
        match q.match_clause.pattern {
            Pattern::VariableLength {
                from, edge, min_hops, max_hops, to,
            } => {
                assert_eq!(from.label.as_deref(), Some("Function"));
                assert_eq!(edge.kind.as_deref(), Some("CALLS"));
                assert_eq!(min_hops, 1);
                assert_eq!(max_hops, 3);
                assert_eq!(to.var, "m");
            }
            other => panic!("expected VariableLength, got {other:?}"),
        }
    }

    #[test]
    fn parse_where_starts_with() {
        let q = parse_cypher(
            "MATCH (n:Function) WHERE n.name STARTS WITH 'foo' RETURN n",
        )
        .unwrap();
        let w = q.where_clause.expect("WHERE clause present");
        match w {
            Expr::StartsWith { var, prop, value } => {
                assert_eq!(var, "n");
                assert_eq!(prop, "name");
                assert_eq!(value, Literal::Str("foo".into()));
            }
            other => panic!("expected StartsWith, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_optional_match() {
        let err = parse_cypher("OPTIONAL MATCH (n) RETURN n").unwrap_err();
        assert!(
            err.contains("OPTIONAL MATCH not supported"),
            "expected rejection message, got: {err}"
        );
    }

    #[test]
    fn parse_return_count_alias() {
        let q = parse_cypher("MATCH (n:Function) RETURN COUNT(*) AS total")
            .unwrap();
        assert_eq!(q.return_clause.items.len(), 1);
        let item = &q.return_clause.items[0];
        assert!(matches!(item.expr, ReturnExpr::CountStar));
        assert_eq!(item.alias.as_deref(), Some("total"));
    }
}
