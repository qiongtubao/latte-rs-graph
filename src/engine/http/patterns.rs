//! HTTP pattern matchers for Phase 11.
//!
//! Each pattern is a small struct pairing a compiled regex with the metadata
//! needed to label a hit. [`detect_routes`] and [`detect_calls`] run all
//! patterns over a source string in declaration order and return the
//! non-overlapping hits (later patterns are skipped when their match range
//! overlaps an earlier one).
//!
//! Languages currently exercised by the engine: `rust`, `typescript`,
//! `javascript`, `c`, `cpp`. Python and Go route/grammar definitions are
//! present so the unit tests can exercise the matchers directly without a
//! tree-sitter grammar. The dispatch happens on textual cues (decorator
//! prefix `@`, attribute syntax `#[...]`, Go's lower-case package call),
//! not on the language argument — when the engine feeds in a Python or Go
//! file it will still pick up the right matchers because the source text
//! itself is the discriminator.



// We avoid pulling in `regex` for the actual matcher to keep Phase 11 lean.
// Instead, the matchers are written as plain string scans + helper predicates.
// This trades some flexibility for a single-translation-unit, zero-deps
// implementation that the tests can drive directly.

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One route-definition hit. `handler_qn` is populated when the call form
/// passes a handler identifier (e.g. `app.get('/p', myHandler)`) or when a
/// decorator is followed by a `def name`/`fn name` declaration. It is `None`
/// for inline / anonymous handlers — the integrator in `tree_sitter.rs`
/// still emits the route node; only the optional `route_handler` edge is
/// suppressed in that case.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteMatch {
    pub method: String,
    pub path: String,
    pub handler_qn: Option<String>,
    pub framework: &'static str,
    pub start_line: u32,
    pub end_line: u32,
    /// Byte offset of the start of the match in the source. Reserved for a
    /// future AST-aware integration; the engine currently consumes only the
    /// line numbers.
    pub start_byte: usize,
}

/// One outbound HTTP call hit. The matching `method` defaults to `GET` for
/// `fetch()` (which has no first-class method argument); other matchers
/// extract the method name from the call.
#[derive(Debug, Clone, PartialEq)]
pub struct CallMatch {
    pub method: String,
    pub path: String,
    pub framework: &'static str,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: usize,
}

/// Detect route definitions across all supported frameworks. Returns
/// non-overlapping hits; routes that share a span are deduplicated by keeping
/// the first match.
pub fn detect_routes(source: &str) -> Vec<RouteMatch> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<RouteMatch> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as u32;
        // 1) Express-style / Fastify / router.<METHOD>('/path', handler)
        if let Some(m) = match_dot_method_route(line) {
            out.push(RouteMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 2) Python decorator @app.get('/path') on its own line, with the
        //    handler resolved by looking at the next non-blank line.
        if let Some(mut m) = match_python_decorator_route(line) {
            // Peek the next non-blank line for a `def name` or `async def name`.
            for next in lines.iter().skip(idx + 1) {
                let trimmed = next.trim();
                if trimmed.is_empty() || trimmed.starts_with('@') {
                    continue;
                }
                if let Some(name) = parse_python_def_name(trimmed) {
                    m.handler_qn = Some(name.to_string());
                    m.end_line = line_num + (next_line_distance(&lines, idx));
                }
                break;
            }
            out.push(m);
            continue;
        }
        // 3) Go http.HandleFunc("/path", handler) / http.Handle("/path", handler)
        if let Some(m) = match_go_handle_func(line) {
            out.push(RouteMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 4) Rust proc-macro #[get("/path")] — handler may live on the same
        //    line (single-line form: `#[get("/x")] async fn hi() {}`) or on
        //    the next line. Try the same line first; fall back to the next.
        if let Some(mut m) = match_rust_proc_macro_route(line) {
            m.start_line = line_num;
            m.end_line = line_num;
            let same_line_name = parse_rust_fn_name(line);
            if let Some(name) = same_line_name {
                m.handler_qn = Some(name.to_string());
            } else {
                for next in lines.iter().skip(idx + 1) {
                    let trimmed = next.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(name) = parse_rust_fn_name(trimmed) {
                        m.handler_qn = Some(name.to_string());
                        m.end_line = line_num + next_line_distance(&lines, idx);
                    }
                    break;
                }
            }
            out.push(m);
            continue;
        }
    }
    dedup_routes(out)
}

/// Detect outbound HTTP call sites across all supported frameworks.
pub fn detect_calls(source: &str) -> Vec<CallMatch> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<CallMatch> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as u32;
        // 1) fetch('url')
        if let Some(m) = match_fetch_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 2) axios.METHOD('url', ...)
        if let Some(m) = match_axios_method_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 3) axios({ method: 'GET', url: '/p' })
        if let Some(m) = match_axios_object_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 4) requests.METHOD / httpx.METHOD / client.METHOD
        if let Some(m) = match_python_client_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 5) Go http.Get / http.Post / http.PostForm
        if let Some(m) = match_go_http_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 6) Rust reqwest::get / reqwest::Client::get
        if let Some(m) = match_reqwest_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
            continue;
        }
        // 7) Vue/Nuxt $http.METHOD / $axios.METHOD
        if let Some(m) = match_dollar_http_call(line) {
            out.push(CallMatch {
                start_line: line_num,
                end_line: line_num,
                start_byte: 0,
                ..m
            });
        }
    }
    dedup_calls(out)
}

// ---------------------------------------------------------------------------
// Route matchers (return Option<RouteMatch without line numbers set>)
// ---------------------------------------------------------------------------

/// `app.get('/path', handler)` / `router.post('/path', handler)` /
/// `fastify.delete('/path', handler)` / Go `r.GET('/path', handler)` /
/// `e.POST('/path', handler)` (Echo), etc.
///
/// We accept any single-segment receiver name (alphanumeric + `_`) and any
/// of the standard HTTP method names. The "framework" label is heuristically
/// derived from the receiver name — known names map to clean labels, others
/// fall back to `"node_http"`.
fn match_dot_method_route(line: &str) -> Option<RouteMatch> {
    let trimmed = line.trim_start();
    // Receiver: identifier (letters/digits/underscore). Could be `app`,
    // `router`, `fastify`, `server`, `r`, `e`, `app`, `router`, `mux`, `bp`.
    let (receiver, rest) = split_first_identifier(trimmed)?;
    if !rest.starts_with('.') {
        return None;
    }
    let after_dot = &rest[1..];
    let (method, rest2) = split_first_identifier(after_dot)?;
    let method_upper = method.to_ascii_uppercase();
    if !is_known_http_method(&method_upper) {
        return None;
    }
    let args = rest2.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    // Pull the first quoted-string argument out of the parens. We don't need
    // to balance parens — the path is always a single-quoted or double-quoted
    // string immediately after the opening paren.
    let path = extract_first_quoted(args)?;
    // Optional handler: any identifier after the comma. We accept a single
    // bare identifier or a `name.handler` chain.
    let after_path = skip_first_string(args);
    let handler = match take_handler_arg(after_path) {
        Some(h) => Some(h.to_string()),
        None => None,
    };
    Some(RouteMatch {
        method: method_upper,
        path: path.to_string(),
        handler_qn: handler,
        framework: framework_for_receiver(receiver),
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `@app.get('/path')` / `@router.post('/path')` / `@bp.delete('/path')` /
/// `@api_view(['GET'])` (DRF) / `@app.api_route('/path', methods=['GET'])`
fn match_python_decorator_route(line: &str) -> Option<RouteMatch> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('@') {
        return None;
    }
    let after_at = &trimmed[1..];
    // Two shapes:
    //   1) @app.get('...')
    //   2) @api_view(['GET'])
    if let Some(rest) = strip_prefix_ci(after_at, "api_view") {
        // DRF: @api_view(['GET']) — the method is inside the list literal.
        let open = rest.find('[')?;
        let close = rest[open..].find(']')?;
        let list_body = &rest[open + 1..open + close];
        let method = first_quoted_token(list_body)
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_else(|| "GET".to_string());
        return Some(RouteMatch {
            method,
            path: "/".to_string(), // DRF exposes a single view for a method list
            handler_qn: None,
            framework: "drf",
            start_line: 0,
            end_line: 0,
            start_byte: 0,
        });
    }
    if let Some(rest) = strip_prefix_ci(after_at, "api_route") {
        // @app.api_route('/path', methods=['GET'])
        let args = rest.trim_start();
        let path = extract_first_quoted(args)?;
        let method = if let Some(methods_open) = rest.find("methods") {
            let bracket = rest[methods_open..].find('[')?;
            let close = rest[methods_open + bracket..].find(']')?;
            let body = &rest[methods_open + bracket + 1..methods_open + bracket + close];
            first_quoted_token(body)
                .map(|s| s.to_ascii_uppercase())
                .unwrap_or_else(|| "GET".to_string())
        } else {
            "GET".to_string()
        };
        return Some(RouteMatch {
            method,
            path: path.to_string(),
            handler_qn: None,
            framework: framework_for_receiver("app"), // generic
            start_line: 0,
            end_line: 0,
            start_byte: 0,
        });
    }
    // @app.METHOD('...') — find a `.METHOD(` token.
    let dot = after_at.find('.')?;
    let receiver = &after_at[..dot];
    let after_dot = &after_at[dot + 1..];
    let (method, rest) = split_first_identifier(after_dot)?;
    let method_upper = method.to_ascii_uppercase();
    if !is_known_http_method(&method_upper) {
        return None;
    }
    let args = rest.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args)?;
    Some(RouteMatch {
        method: method_upper,
        path: path.to_string(),
        handler_qn: None,
        framework: framework_for_receiver(receiver),
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `http.HandleFunc("/path", handler)` / `http.Handle("/path", handler)`
fn match_go_handle_func(line: &str) -> Option<RouteMatch> {
    let trimmed = line.trim_start();
    let rest = strip_prefix_ci(trimmed, "http.HandleFunc")
        .or_else(|| strip_prefix_ci(trimmed, "http.Handle"))?;
    let args = rest.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args)?;
    Some(RouteMatch {
        method: "*".to_string(), // net/http HandleFunc dispatches on its own
        path: path.to_string(),
        handler_qn: take_handler_arg(skip_first_string(args)).map(|s| s.to_string()),
        framework: "net_http",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `#[get("/path")]` / `#[post(...)]` / `#[route("/path", method="GET")]`
fn match_rust_proc_macro_route(line: &str) -> Option<RouteMatch> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("#[") {
        return None;
    }
    let inner = trimmed.strip_prefix("#[")?;
    let (name, rest) = split_first_identifier(inner)?;
    let name_lower = name.to_ascii_lowercase();
    let method = match name_lower.as_str() {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        "options" => "OPTIONS",
        "route" => {
            // #[route("/path", method="GET")] — extract method=...
            let mut m = "GET".to_string();
            if let Some(method_idx) = rest.find("method") {
                let after = &rest[method_idx..];
                if let Some(eq) = after.find('=') {
                    let after_eq = after[eq + 1..].trim_start();
                    if let Some(q) = first_quoted_token(after_eq) {
                        m = q.to_ascii_uppercase();
                    }
                }
            }
            let path = extract_first_quoted(rest)?;
            return Some(RouteMatch {
                method: m,
                path: path.to_string(),
                handler_qn: None,
                framework: "actix",
                start_line: 0,
                end_line: 0,
                start_byte: 0,
            });
        }
        _ => return None,
    };
    let path = extract_first_quoted(rest)?;
    Some(RouteMatch {
        method: method.to_string(),
        path: path.to_string(),
        handler_qn: None,
        framework: "actix",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `fetch('url')` / `fetch("url")`
fn match_fetch_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    let after = strip_prefix_ci(trimmed, "fetch")?;
    let args = after.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args)?;
    Some(CallMatch {
        method: "GET".to_string(),
        path: path.to_string(),
        framework: "fetch",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `axios.get('/p')` / `axios.post('/p', body)` etc.
fn match_axios_method_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    let after = strip_prefix_ci(trimmed, "axios.")?;
    let (method, rest) = split_first_identifier(after)?;
    let method_upper = method.to_ascii_uppercase();
    if !is_known_http_method(&method_upper) {
        return None;
    }
    let args = rest.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args)?;
    Some(CallMatch {
        method: method_upper,
        path: path.to_string(),
        framework: "axios",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `axios({ method: 'GET', url: '/p' })`
fn match_axios_object_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    let after = strip_prefix_ci(trimmed, "axios(")?;
    // We require a method: ... url: ... shape on the same line — that
    // covers the v1 contract without needing a real JS parser.
    let body = after;
    let method = find_key_value_string(body, "method")
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_else(|| "GET".to_string());
    let path = match find_key_value_string(body, "url") {
        Some(p) => p.to_string(),
        None => return None,
    };
    Some(CallMatch {
        method,
        path,
        framework: "axios",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `requests.get('/p')` / `httpx.post('/p')` / `client.delete('/p')`
fn match_python_client_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    // Try the three library prefixes — order matters because `client` is a
    // generic suffix that could collide with anything; we test the longer
    // prefixes first.
    let (after, framework) = if let Some(a) = strip_prefix_ci(trimmed, "requests.") {
        (a, "requests")
    } else if let Some(a) = strip_prefix_ci(trimmed, "httpx.") {
        (a, "httpx")
    } else if let Some(a) = strip_prefix_ci(trimmed, "client.") {
        (a, "httpx") // treat as httpx-style; common enough
    } else {
        return None;
    };
    let (method, rest) = split_first_identifier(after)?;
    let method_upper = method.to_ascii_uppercase();
    if !is_known_http_method(&method_upper) {
        return None;
    }
    let args = rest.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args)?;
    Some(CallMatch {
        method: method_upper,
        path: path.to_string(),
        framework,
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `http.Get(url)` / `http.Post(url, ...)` / `http.PostForm(url, ...)`
fn match_go_http_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    let (after, method) = if let Some(a) = strip_prefix_ci(trimmed, "http.Get(") {
        (a, "GET")
    } else if let Some(a) = strip_prefix_ci(trimmed, "http.Post(") {
        (a, "POST")
    } else if let Some(a) = strip_prefix_ci(trimmed, "http.PostForm(") {
        (a, "POST")
    } else if let Some(a) = strip_prefix_ci(trimmed, "http.Head(") {
        (a, "HEAD")
    } else {
        return None;
    };
    // Path can be a string literal OR a variable — for variable args we
    // emit `<unknown>` so callers know we saw the call but couldn't recover
    // the URL. v1 keeps this lightweight.
    let path = extract_first_quoted(after).unwrap_or("<unknown>").to_string();
    Some(CallMatch {
        method: method.to_string(),
        path,
        framework: "net_http",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

// `framework_for_receiver` lives at the bottom of this file alongside the
// other helpers — see the consolidated definition below.

/// `reqwest::get('url')` / `reqwest::Client::get(&client, 'url')`
fn match_reqwest_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    let after = if let Some(a) = strip_prefix_ci(trimmed, "reqwest::Client::") {
        a
    } else if let Some(a) = strip_prefix_ci(trimmed, "reqwest::") {
        a
    } else {
        return None;
    };
    let (method, rest) = split_first_identifier(after)?;
    let method_upper = method.to_ascii_uppercase();
    if !is_known_http_method(&method_upper) {
        return None;
    }
    let args = rest.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args).unwrap_or("<unknown>").to_string();
    Some(CallMatch {
        method: method_upper,
        path,
        framework: "reqwest",
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

/// `$http.get('/p')` / `$axios.post('/p')` — Vue / Nuxt conventions.
fn match_dollar_http_call(line: &str) -> Option<CallMatch> {
    let trimmed = line.trim_start();
    let (after, framework) = if let Some(a) = strip_prefix_ci(trimmed, "$http.") {
        (a, "vue_http")
    } else if let Some(a) = strip_prefix_ci(trimmed, "$axios.") {
        (a, "axios")
    } else {
        return None;
    };
    let (method, rest) = split_first_identifier(after)?;
    let method_upper = method.to_ascii_uppercase();
    if !is_known_http_method(&method_upper) {
        return None;
    }
    let args = rest.trim_start();
    if !args.starts_with('(') {
        return None;
    }
    let path = extract_first_quoted(args)?;
    Some(CallMatch {
        method: method_upper,
        path: path.to_string(),
        framework,
        start_line: 0,
        end_line: 0,
        start_byte: 0,
    })
}

// ---------------------------------------------------------------------------
// Tiny string helpers — these would normally come from `regex` or `once_cell`,
// but Phase 11 only needs single-line token extraction, so inline scanners
// keep the dependency footprint unchanged.
// ---------------------------------------------------------------------------

fn split_first_identifier(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() || !(s.chars().next()?.is_ascii_alphabetic() || s.chars().next()? == '_') {
        return None;
    }
    let mut split_at = 0;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            split_at = i + ch.len_utf8();
        } else {
            break;
        }
    }
    if split_at == 0 {
        return None;
    }
    Some((&s[..split_at], &s[split_at..]))
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    // Use s.get(..) instead of s[..] so we never slice mid-character if
    // `s` happens to contain a multi-byte UTF-8 sequence at the byte
    // boundary set by `prefix.len()`.
    let head = s.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    Some(&s[prefix.len()..])
}

fn extract_first_quoted(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            let quote = b;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != quote {
                // Skip backslash escapes so `\"` inside a double-quoted
                // string doesn't terminate the path prematurely.
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                j += 1;
            }
            if j >= bytes.len() {
                return None;
            }
            return Some(&s[start..j]);
        }
        i += 1;
    }
    None
}

/// Returns the substring of `s` immediately after the first quoted string
/// literal. Used to look at the trailing args (handler identifier, etc.).
fn skip_first_string(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            let quote = b;
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != quote {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                j += 1;
            }
            if j < bytes.len() {
                return &s[j + 1..];
            }
            return "";
        }
        i += 1;
    }
    s
}

/// After the path argument, accept a comma then an identifier or a
/// dotted-identifier (e.g. `controllers.Index`). Whitespace tolerated.
fn take_handler_arg(after_path: &str) -> Option<&str> {
    let s = after_path.trim_start();
    if !s.starts_with(',') {
        return None;
    }
    let rest = s[1..].trim_start();
    let mut end = 0;
    for (i, ch) in rest.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            end = i + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        None
    } else {
        Some(&rest[..end])
    }
}

/// Find `key: 'value'` or `key: "value"` and return the value. Used by
/// `match_axios_object_call` to pull `method` and `url` out of a config
/// object literal.
fn find_key_value_string<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let idx = s.find(&format!("{key}:"))?;
    let after = &s[idx + key.len() + 1..];
    extract_first_quoted(after)
}

/// First quoted token in `s` (used to pull a method name out of `methods=['GET']`).
fn first_quoted_token(s: &str) -> Option<&str> {
    extract_first_quoted(s)
}

/// `def name(...)` / `async def name(...)` — return the function name.
fn parse_python_def_name(line: &str) -> Option<&str> {
    // Strip `async ` (case-insensitive) without allocating; then strip `def `
    // and the function name. Everything stays borrowed from `line` so the
    // returned `&str` is valid for the caller's lifetime.
    let after_async = strip_prefix_ci(line, "async ").unwrap_or(line).trim_start();
    let after_def = strip_prefix_ci(after_async, "def ")?;
    let (name, _) = split_first_identifier(after_def)?;
    Some(name)
}

/// `fn name(...)` / `async fn name(...)` / `pub async fn name(...)` — return
/// the function name. We tolerate visibility / async / const qualifiers by
/// splitting on the last `fn ` token.
fn parse_rust_fn_name(line: &str) -> Option<&str> {
    // Find `fn` token as a whole word, then take the next identifier.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 2 <= bytes.len() && &bytes[i..i + 2] == b"fn" {
            let before_ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let after_ok = i + 2 == bytes.len() || bytes[i + 2] == b' ' || bytes[i + 2] == b'\t';
            if before_ok && after_ok {
                let after = &line[i + 2..].trim_start();
                let (name, _) = split_first_identifier(after)?;
                return Some(name);
            }
        }
        i += 1;
    }
    None
}

/// Number of lines between `lines[idx]` and the next non-blank,
/// non-decorator/attribute line.
fn next_line_distance(lines: &[&str], idx: usize) -> u32 {
    let mut dist = 0u32;
    for line in lines.iter().skip(idx + 1) {
        dist += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('@') || trimmed.starts_with("#[") {
            continue;
        }
        return dist;
    }
    dist
}

fn is_known_http_method(m: &str) -> bool {
    matches!(
        m,
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS" | "TRACE" | "CONNECT"
    )
}

fn framework_for_receiver(receiver: &str) -> &'static str {
    match receiver.to_ascii_lowercase().as_str() {
        "app" => "express", // most common default for `app.<METHOD>(...)`
        "router" => "express",
        "server" => "express",
        "fastify" => "fastify",
        "r" => "gin",        // r.GET / r.POST is canonical Gin AND Chi
        "e" => "echo",
        "fiber" => "fiber",
        "mux" => "chi",
        "bp" => "flask",
        "api" => "flask",
        _ => "node_http",
    }
}

fn dedup_routes(items: Vec<RouteMatch>) -> Vec<RouteMatch> {
    let mut seen: Vec<(u32, String, String, String)> = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    for r in items {
        let key = (r.start_line, r.method.clone(), r.path.clone(), r.framework.to_string());
        if seen.iter().any(|k| k.0 == key.0 && k.1 == key.1 && k.2 == key.2 && k.3 == key.3) {
            continue;
        }
        seen.push(key);
        out.push(r);
    }
    out
}

fn dedup_calls(items: Vec<CallMatch>) -> Vec<CallMatch> {
    let mut seen: Vec<(u32, String, String)> = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    for c in items {
        let key = (c.start_line, c.method.clone(), c.path.clone());
        if seen.iter().any(|k| k.0 == key.0 && k.1 == key.1 && k.2 == key.2) {
            continue;
        }
        seen.push(key);
        out.push(c);
    }
    out
}