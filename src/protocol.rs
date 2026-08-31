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
}
