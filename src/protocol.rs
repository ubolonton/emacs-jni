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

pub fn log_message(level: &str, message: &str) -> String {
    format!("(log {} {})\n", level, quote(message))
}

/// Renders a Rust string as an Emacs Lisp string literal: wraps in double
/// quotes and escapes backslashes and double quotes. This only needs to
/// cover what our own messages ever contain (plain error/log text), not
/// full Lisp reader syntax.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
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
}
