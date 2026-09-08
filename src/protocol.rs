use crate::status::Status;

pub fn status_message(status: Status, message: Option<&str>) -> String {
    let msg_sexp = match message {
        Some(m) => quote(m),
        None => "nil".to_string(),
    };
    format!("(status {} {})\n", status.as_tag(), msg_sexp)
}

pub fn load_jar_done_message(id: u64, ok: bool, err: Option<&str>) -> String {
    let ok_sym = if ok { "t" } else { "nil" };
    let err_sexp = match err {
        Some(e) => quote(e),
        None => "nil".to_string(),
    };
    format!("(load-jar-done {} {} {})\n", id, ok_sym, err_sexp)
}

pub fn call_done_message(id: u64, ok: bool, result: Option<&str>, err: Option<&str>) -> String {
    let ok_sym = if ok { "t" } else { "nil" };
    let result_sexp = result.unwrap_or("nil");
    let err_sexp = match err {
        Some(e) => quote(e),
        None => "nil".to_string(),
    };
    format!("(call-done {id} {ok_sym} {result_sexp} {err_sexp})\n")
}

/// ARGS is a pre-formatted S-expression list fragment, e.g. `"(1 2)"` or
/// `"nil"` for zero args — produced by Task 4's marshalling code.
pub fn invoke_message(id: u64, handle: u64, args: &str) -> String {
    format!("(invoke {id} {handle} {args})\n")
}

pub fn release_message(handle: u64) -> String {
    format!("(release {handle})\n")
}

/// Wakes the main thread up to drain the Lisp-access request queue
/// (`lisp_requests::service`, via `jvm--service-lisp-requests`) — carries
/// no payload, since the actual request rides the in-process queue, not
/// the wire (see design spec, "Request/reply flow").
pub fn lisp_request_message() -> String {
    "(lisp-request)\n".to_string()
}

pub fn log_message(level: &str, message: &str) -> String {
    format!("(log {} {})\n", level, quote(message))
}

/// Renders a Rust string as an Emacs Lisp string literal: wraps in double
/// quotes and escapes backslashes, double quotes, and raw newlines/carriage
/// returns. This only needs to cover what our own messages ever contain
/// (plain error/log text), not full Lisp reader syntax.
///
/// Escaping `\n`/`\r` (not just `"`/`\`) matters because the wire protocol
/// is newline-delimited: `jvm--process-filter` (elisp/emacs-jni.el) splits
/// incoming bytes into one message per literal `\n` *before* calling `read`
/// on each line. A raw newline embedded inside a quoted string here — e.g.
/// a multi-line Java exception's `to_string()`, which a `CaughtJavaException`
/// stack trace genuinely can produce (found via Task 6's callback-wrapping
/// test, whose triggering `InvocationTargetException` rendered several
/// `\n`-separated stack frames into what `call_done_message` sends) — would
/// fracture one wire message into multiple malformed fragments, each
/// failing `read` with "End of file during parsing". Emacs Lisp's string
/// reader recognizes `\n`/`\r` as the corresponding control character (the
/// same escape syntax as C), so emitting the two-character escape sequence
/// here keeps the wire single-line while still round-tripping correctly
/// into the original multi-line string on the Lisp side.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_simple_status() {
        assert_eq!(status_message(Status::Ready, None), "(status ready nil)\n");
    }

    #[test]
    fn formats_status_with_message() {
        assert_eq!(
            status_message(Status::StartFailed, Some("boom")),
            "(status start-failed \"boom\")\n"
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes_in_message() {
        assert_eq!(
            status_message(Status::Crashed, Some("bad \"quote\" and \\slash")),
            "(status crashed \"bad \\\"quote\\\" and \\\\slash\")\n"
        );
    }

    #[test]
    fn escapes_embedded_newlines_and_carriage_returns_so_the_wire_stays_one_line() {
        // A raw '\n' in the rendered message would be mistaken by
        // `jvm--process-filter`'s line-splitting for the end of the wire
        // message itself -- see `quote`'s doc comment. The whole point of
        // this test is that the *rendered string* contains no raw '\n' or
        // '\r' byte anywhere, not just that it round-trips semantically.
        let rendered = status_message(Status::Crashed, Some("line one\nline two\rline three"));
        assert_eq!(rendered, "(status crashed \"line one\\nline two\\rline three\")\n");
        // Exactly one '\n' in the whole rendered message: the trailing
        // message terminator `call_done_message`/`status_message` always
        // append, not any newline from the original content.
        assert_eq!(rendered.matches('\n').count(), 1);
        assert!(!rendered.contains('\r'));
    }

    #[test]
    fn formats_load_jar_done_success() {
        assert_eq!(load_jar_done_message(3, true, None), "(load-jar-done 3 t nil)\n");
    }

    #[test]
    fn formats_load_jar_done_failure() {
        assert_eq!(
            load_jar_done_message(3, false, Some("not found")),
            "(load-jar-done 3 nil \"not found\")\n"
        );
    }

    #[test]
    fn formats_log_message() {
        assert_eq!(log_message("info", "hello"), "(log info \"hello\")\n");
    }

    #[test]
    fn formats_call_done_success() {
        assert_eq!(
            call_done_message(7, true, Some("42"), None),
            "(call-done 7 t 42 nil)\n"
        );
    }

    #[test]
    fn formats_call_done_failure() {
        assert_eq!(
            call_done_message(7, false, None, Some("boom")),
            "(call-done 7 nil nil \"boom\")\n"
        );
    }

    #[test]
    fn formats_invoke_with_args() {
        assert_eq!(invoke_message(3, 9, "(1 2)"), "(invoke 3 9 (1 2))\n");
    }

    #[test]
    fn formats_invoke_with_no_args() {
        assert_eq!(invoke_message(3, 9, "nil"), "(invoke 3 9 nil)\n");
    }

    #[test]
    fn formats_release() {
        assert_eq!(release_message(9), "(release 9)\n");
    }

    #[test]
    fn formats_lisp_request() {
        assert_eq!(lisp_request_message(), "(lisp-request)\n");
    }
}
