//! Recursive-descent parser for the Cypher v1 subset.
//!
//! Grammar:
//! ```text
//! query     := query_core ( UNION [ALL] query_core )?
//! query_core:= MATCH pattern [WHERE expr]
//!              ( OPTIONAL MATCH pattern [WHERE expr] )*
//!              RETURN return_clause
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
//! than silently skipped. v1-specific limits (enforced by this parser):
//! - exactly one main MATCH, no chained `MATCH` after `OPTIONAL MATCH`.
//! - 3+ way UNION chains (`a UNION b UNION c`) are rejected.
//! - the right-hand side of a UNION MUST NOT contain another UNION
//!   chain (depth 2 only).
//! - `WHERE` is forbidden after `UNION` (it would otherwise belong to
//!   the right-hand side without an obvious anchor).

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
        // Top-level keyword guard — leading UNION / OPTIONAL MATCH /
        // WITH / UNWIND have nowhere to attach at the top of a query.
        match self.peek() {
            Some(Token::Optional) => {
                return Err(
                    "OPTIONAL MATCH must follow a MATCH in the same query".to_string(),
                );
            }
            Some(Token::Union) => {
                return Err("UNION must follow a complete query".to_string())
            }
            Some(Token::Unwind) => {
                return Err("UNWIND not supported in v1".to_string())
            }
            Some(Token::With) => {
                return Err("WITH not supported in v1".to_string())
            }
            _ => {}
        }
        let mut left = self.parse_query_core()?;

        // Optional UNION [ALL] <right> — depth 2 only.
        if matches!(self.peek(), Some(Token::Union)) {
            self.eat(); // UNION
            let union_all = if matches!(self.peek(), Some(Token::All)) {
                self.eat();
                true
            } else {
                false
            };
            let right = self.parse_query_core()?;
            // Reject 3+ way unions by checking that the right side
            // didn't itself consume a UNION token. The parser would
            // have stopped at end-of-input or at the trailing-token
            // check below, so we just confirm `right.union_next` is
            // None (parse_query_core never recurses for unions).
            if right.union_next.is_some() {
                return Err(
                    "UNION chains longer than 2 are not supported in v1".to_string(),
                );
            }
            // WHERE is allowed inside the right side's own MATCH /
            // OPTIONAL MATCH clause — the "no WHERE on UNION" rule
            // refers to a bare WHERE between UNION and the next
            // MATCH (which would have nowhere to attach). The
            // parse_query_core parser never produces such a WHERE
            // (it always binds WHERE to the preceding MATCH), so we
            // don't need an explicit rejection here.
            left = CypherQuery {
                match_clauses: left.match_clauses,
                optional_clauses: left.optional_clauses,
                return_clause: left.return_clause,
                order_by: left.order_by,
                limit: left.limit,
                union_next: Some(Box::new(right)),
                union_all,
            };
        }

        Ok(left)
    }

    /// Parse a single non-union query: MATCH (plus optional OPTIONAL
    /// MATCH chain) + RETURN + ORDER BY + LIMIT.
    fn parse_query_core(&mut self) -> Result<CypherQuery, String> {
        // 1. MATCH (the only mandatory leading clause).
        self.expect_keyword(&Token::Match)?;
        let pattern = self.parse_pattern()?;
        let main_where = if matches!(self.peek(), Some(Token::Where)) {
            self.eat();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let match_clauses = vec![MatchClause { pattern, where_clause: main_where }];

        // 2. Zero or more OPTIONAL MATCH clauses.
        let mut optional_clauses: Vec<MatchClause> = Vec::new();
        while matches!(self.peek(), Some(Token::Optional)) {
            self.eat(); // OPTIONAL
            self.expect_keyword(&Token::Match)?;
            let p = self.parse_pattern()?;
            let w = if matches!(self.peek(), Some(Token::Where)) {
                self.eat();
                Some(self.parse_expr()?)
            } else {
                None
            };
            optional_clauses.push(MatchClause { pattern: p, where_clause: w });
        }

        // 3. Once we leave the OPTIONAL MATCH chain, the next token MUST
        // be RETURN — a stray bare MATCH is rejected as in v1.
        if matches!(self.peek(), Some(Token::Match)) {
            return Err(
                "multiple MATCH clauses are not supported in v1 — use OPTIONAL MATCH \
                 instead"
                    .to_string(),
            );
        }

        // 4. RETURN
        if !matches!(self.peek(), Some(Token::Return)) {
            return Err(format!(
                "expected RETURN but found {:?}",
                self.peek()
            ));
        }
        self.eat();
        let return_clause = self.parse_return_clause()?;

        // 5. Optional ORDER BY
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

        // 6. Optional LIMIT
        let limit = if matches!(self.peek(), Some(Token::Limit)) {
            self.eat();
            match self.eat() {
                Some(Token::IntLit(n)) if n > 0 => Some(n as u32),
                Some(Token::IntLit(n)) => {
                    return Err(format!("LIMIT must be positive, got {n}"))
                }
                other => {
                    return Err(format!(
                        "expected positive integer after LIMIT, found {:?}",
                        other
                    ))
                }
            }
        } else {
            None
        };

        Ok(CypherQuery {
            match_clauses,
            optional_clauses,
            return_clause,
            order_by,
            limit,
            union_next: None,
            union_all: false,
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
        assert!(matches!(q.match_clauses[0].pattern, Pattern::SingleNode(_)));
        assert_eq!(q.return_clause.items.len(), 1);
        assert!(matches!(
            q.return_clause.items[0].expr,
            ReturnExpr::Var(_)
        ));
        assert!(q.match_clauses[0].where_clause.is_none());
        assert!(q.limit.is_none());
    }

    #[test]
    fn parse_relationship() {
        let q =
            parse_cypher("MATCH (n:Function)-[:CALLS]->(m) RETURN n, m").unwrap();
        match &q.match_clauses[0].pattern {
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
        match &q.match_clauses[0].pattern {
            Pattern::VariableLength {
                from, edge, min_hops, max_hops, to,
            } => {
                assert_eq!(from.label.as_deref(), Some("Function"));
                assert_eq!(edge.kind.as_deref(), Some("CALLS"));
                assert_eq!(*min_hops, 1);
                assert_eq!(*max_hops, 3);
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
        let w = q.match_clauses[0].where_clause.as_ref().expect("WHERE clause present");
        match w {
            Expr::StartsWith { var, prop, value } => {
                assert_eq!(var, "n");
                assert_eq!(prop, "name");
                assert_eq!(value, &Literal::Str("foo".into()));
            }
            other => panic!("expected StartsWith, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_leading_optional_match() {
        let err = parse_cypher("OPTIONAL MATCH (n) RETURN n").unwrap_err();
        assert!(
            err.contains("OPTIONAL MATCH must follow a MATCH"),
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
    #[test]
    fn parse_optional_match_single_node() {
        let q = parse_cypher("MATCH (n) OPTIONAL MATCH (m) RETURN n, m").unwrap();
        assert_eq!(q.match_clauses.len(), 1);
        assert_eq!(q.optional_clauses.len(), 1);
        // The main MATCH is the single-node `(n)`; the optional side is `(m)`.
        assert!(matches!(q.match_clauses[0].pattern, Pattern::SingleNode(_)));
        assert!(matches!(
            q.optional_clauses[0].pattern,
            Pattern::SingleNode(_)
        ));
        // No UNION.
        assert!(q.union_next.is_none());
        assert_eq!(q.return_clause.items.len(), 2);
    }

    #[test]
    fn parse_optional_match_with_where() {
        let q = parse_cypher(
            "MATCH (n:Function) WHERE n.name = 'foo' \
             OPTIONAL MATCH (m) WHERE m.name = 'bar' RETURN n, m",
        )
        .unwrap();
        assert!(q.match_clauses[0].where_clause.is_some());
        assert!(q.optional_clauses[0].where_clause.is_some());
        // Make sure the two WHEREs landed on the right clauses (they
        // must NOT be cross-applied).
        match q.match_clauses[0].where_clause.as_ref().unwrap() {
            Expr::BinaryOp { left, right, .. } => {
                if let Expr::Property { var, prop } = left.as_ref() {
                    assert_eq!(var, "n");
                    assert_eq!(prop, "name");
                } else {
                    panic!("main WHERE left is not a property on n");
                }
                if let Expr::Literal(Literal::Str(s)) = right.as_ref() {
                    assert_eq!(s, "foo");
                } else {
                    panic!("main WHERE right is not 'foo'");
                }
            }
            other => panic!("main WHERE is not BinaryOp, got {other:?}"),
        }
        match q.optional_clauses[0].where_clause.as_ref().unwrap() {
            Expr::BinaryOp { left, right, .. } => {
                if let Expr::Property { var, prop } = left.as_ref() {
                    assert_eq!(var, "m");
                    assert_eq!(prop, "name");
                } else {
                    panic!("optional WHERE left is not a property on m");
                }
                if let Expr::Literal(Literal::Str(s)) = right.as_ref() {
                    assert_eq!(s, "bar");
                } else {
                    panic!("optional WHERE right is not 'bar'");
                }
            }
            other => panic!("optional WHERE is not BinaryOp, got {other:?}"),
        }
    }

    #[test]
    fn parse_union_distinct() {
        let q = parse_cypher("MATCH (n) RETURN n UNION MATCH (m) RETURN m")
            .unwrap();
        let right = q.union_next.as_ref().expect("UNION right side");
        assert!(!q.union_all, "UNION (without ALL) is distinct");
        assert!(right.union_next.is_none(), "depth 2 only");
        assert_eq!(right.match_clauses.len(), 1);
        assert!(right.optional_clauses.is_empty());
    }

    #[test]
    fn parse_union_all() {
        let q = parse_cypher("MATCH (n) RETURN n UNION ALL MATCH (m) RETURN m")
            .unwrap();
        assert!(q.union_all, "UNION ALL must set union_all=true");
        let right = q.union_next.as_ref().expect("UNION right side");
        assert_eq!(right.match_clauses.len(), 1);
    }

    #[test]
    fn parse_rejects_optional_then_match() {
        let err = parse_cypher(
            "MATCH (n) OPTIONAL MATCH (m) MATCH (x) RETURN n",
        )
        .unwrap_err();
        // The parser's anti-pattern guard fires once an OPTIONAL MATCH
        // chain ends and the next token is a bare MATCH.
        assert!(
            err.contains("multiple MATCH clauses")
                || err.contains("OPTIONAL MATCH"),
            "expected rejection message, got: {err}"
        );
    }

    #[test]
    fn parse_rejects_3way_union() {
        let err = parse_cypher(
            "MATCH (n) RETURN n UNION MATCH (m) RETURN m UNION MATCH (x) RETURN x",
        )
        .unwrap_err();
        // The outer parser stops at the trailing UNION token; either
        // the explicit "longer than 2" message or the trailing-token
        // message is acceptable as proof that 3-way is rejected.
        assert!(
            err.contains("UNION")
                || err.contains("unexpected trailing"),
            "expected rejection of 3-way UNION, got: {err}"
        );
    }
}

