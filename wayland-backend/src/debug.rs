//! Debugging helpers to handle `WAYLAND_DEBUG` env variable.

use std::{
    fmt::{self, Display, Write as _},
    io::Write as _,
    os::unix::io::AsRawFd,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::protocol::Argument;

/// The `WAYLAND_DEBUG` env variable is set to debug client.
pub fn has_debug_client_env() -> bool {
    matches!(std::env::var_os("WAYLAND_DEBUG"), Some(str) if str == "1" || str == "client")
}

/// Print the dispatched message to stderr in a following format:
///
/// `[timestamp] <- interface@id.msg_name(args)`
#[cfg_attr(unstable_coverage, coverage(off))]
pub fn print_dispatched_message<Id: Display, Fd: AsRawFd>(
    interface: &str,
    id: u32,
    msg_name: &str,
    args: &[Argument<Id, Fd>],
) {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "{} <- {}@{}.{}, ({})",
        Timestamp,
        interface,
        id,
        msg_name,
        DisplaySlice(args)
    );
    write_stderr(&line);
}

/// Print the send message to stderr in a following format:
///
/// `[timestamp] -> interface@id.msg_name(args)`
#[cfg_attr(unstable_coverage, coverage(off))]
pub fn print_send_message<Id: Display, Fd: AsRawFd>(
    interface: &str,
    id: u32,
    msg_name: &str,
    args: &[Argument<Id, Fd>],
    discarded: bool,
) {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "{}{} -> {}@{}.{}({})",
        Timestamp,
        if discarded { "[discarded]" } else { "" },
        interface,
        id,
        msg_name,
        DisplaySlice(args)
    );
    write_stderr(&line);
}

/// One write, errors ignored. No panic on a closed stderr.
fn write_stderr(line: &str) {
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

pub(crate) struct DisplaySlice<'a, D>(pub &'a [D]);

impl<D: Display> Display for DisplaySlice<'_, D> {
    #[cfg_attr(unstable_coverage, coverage(off))]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut it = self.0.iter();
        if let Some(val) = it.next() {
            write!(f, "{val}")?;
        }
        for val in it {
            write!(f, ", {val}")?;
        }
        Ok(())
    }
}

/// ms.us timestamp, tagged `[rs]`
struct Timestamp;

impl Display for Timestamp {
    #[cfg_attr(unstable_coverage, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(timestamp) = SystemTime::now().duration_since(UNIX_EPOCH) {
            // NOTE this is all to make timestamps the same with libwayland, so the log doesn't look
            // out of place when sys tries to log on their own.
            let time =
                (timestamp.as_secs() * 1000000 + timestamp.subsec_nanos() as u64 / 1000) as u32;
            write!(f, "[{:7}.{:03}][rs]", time / 1000, time % 1000)?;
        }
        Ok(())
    }
}

/// libwayland 1.26 timestamp: `[HH:MM:SS.uuuuuu]`
#[cfg(feature = "log")]
struct ClockTime;

#[cfg(feature = "log")]
impl Display for ClockTime {
    #[cfg_attr(unstable_coverage, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = timestamp.as_secs();
        write!(
            f,
            "[{:02}:{:02}:{:02}.{:06}]",
            (secs / 3600) % 24,
            (secs / 60) % 60,
            secs % 60,
            timestamp.subsec_micros()
        )
    }
}

/// Wire trace target. One `trace` record per message, libwayland 1.26
/// format, ` <N>` connection tag (client serial).
#[cfg(feature = "log")]
pub(crate) const WIRE_TARGET: &str = "wayland_backend::wire";

/// Only format when the target is enabled
#[cfg(feature = "log")]
pub(crate) fn wire_enabled() -> bool {
    #[cfg(test)]
    if WIRE_CAPTURE.with(|c| c.borrow().is_some()) {
        return true;
    }
    log::log_enabled!(target: WIRE_TARGET, log::Level::Trace)
}

#[cfg(all(test, feature = "log"))]
thread_local! {
    /// Test capture. `Some` enables the trace on this thread.
    pub(crate) static WIRE_CAPTURE: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// Message arg, printed as libwayland does
#[cfg(feature = "log")]
pub(crate) enum WireArg<'a> {
    Int(i32),
    Uint(u32),
    Fixed(i32),
    Str(Option<&'a std::ffi::CStr>),
    /// `None` is null
    Object(Option<(&'a str, u32)>),
    /// `None` interface: untyped new_id (wl_registry.bind)
    NewId(Option<&'a str>, u32),
    /// byte count
    Array(usize),
    Fd(i32),
}

#[cfg(feature = "log")]
impl Display for WireArg<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Uint(value) => write!(f, "{value}"),
            // 390625 is 1e8 / 256
            Self::Fixed(value) if value >= 0 => {
                write!(f, "{}.{:08}", value / 256, 390625 * (value % 256))
            }
            Self::Fixed(value) => {
                write!(f, "-{}.{:08}", -(value / 256), -390625 * (value % 256))
            }
            Self::Str(Some(value)) => write_quoted(f, &value.to_string_lossy()),
            Self::Str(None) | Self::Object(None) => f.write_str("nil"),
            Self::Object(Some((interface, id))) => write!(f, "{interface}#{id}"),
            Self::NewId(interface, id) => {
                write!(f, "new id {}#", interface.unwrap_or("[unknown]"))?;
                if id == 0 { f.write_str("nil") } else { write!(f, "{id}") }
            }
            Self::Array(len) => write!(f, "array[{len}]"),
            Self::Fd(fd) => write!(f, "fd {fd}"),
        }
    }
}

/// Quoted and escaped, unlike libwayland. A client string can't end the
/// line or forge one.
#[cfg(feature = "log")]
fn write_quoted(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in value.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            c if c.is_control() => write!(f, "\\x{:02x}", c as u32)?,
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

/// `[HH:MM:SS.uuuuuu] <conn>  -> iface#id.msg(args)`, no arrow if received
#[cfg(feature = "log")]
pub(crate) struct WireLine<'a> {
    pub(crate) conn: u32,
    pub(crate) sent: bool,
    pub(crate) interface: &'a str,
    pub(crate) id: u32,
    pub(crate) message: &'a str,
    pub(crate) args: &'a [WireArg<'a>],
}

#[cfg(feature = "log")]
impl Display for WireLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} <{}> {}{}#{}.{}({})",
            ClockTime,
            self.conn,
            if self.sent { " -> " } else { "" },
            self.interface,
            self.id,
            self.message,
            DisplaySlice(self.args)
        )
    }
}

/// Log a wire trace line
#[cfg(feature = "log")]
pub(crate) fn log_wire(line: &WireLine<'_>) {
    #[cfg(test)]
    WIRE_CAPTURE.with(|c| {
        if let Some(lines) = c.borrow_mut().as_mut() {
            lines.push(line.to_string());
        }
    });
    log::trace!(target: WIRE_TARGET, "{line}");
}

#[cfg(all(test, feature = "log"))]
pub(crate) mod tests {
    use super::*;

    fn arg(a: WireArg<'_>) -> String {
        a.to_string()
    }

    #[test]
    fn fixed_prints_as_libwayland_does() {
        assert_eq!(arg(WireArg::Fixed(256)), "1.00000000");
        assert_eq!(arg(WireArg::Fixed(384)), "1.50000000");
        assert_eq!(arg(WireArg::Fixed(9823)), "38.37109375");
        assert_eq!(arg(WireArg::Fixed(-384)), "-1.50000000");
        assert_eq!(arg(WireArg::Fixed(-1)), "-0.00390625");
        assert_eq!(arg(WireArg::Fixed(i32::MIN)), "-8388608.00000000");
    }

    #[test]
    fn objects_and_new_ids() {
        assert_eq!(arg(WireArg::Object(Some(("wl_surface", 7)))), "wl_surface#7");
        assert_eq!(arg(WireArg::Object(None)), "nil");
        assert_eq!(arg(WireArg::NewId(Some("wl_callback"), 3)), "new id wl_callback#3");
        assert_eq!(arg(WireArg::NewId(None, 3)), "new id [unknown]#3");
        assert_eq!(arg(WireArg::NewId(Some("wl_callback"), 0)), "new id wl_callback#nil");
    }

    #[test]
    fn other_args() {
        assert_eq!(arg(WireArg::Int(-13)), "-13");
        assert_eq!(arg(WireArg::Uint(u32::MAX)), "4294967295");
        assert_eq!(arg(WireArg::Array(9)), "array[9]");
        assert_eq!(arg(WireArg::Fd(5)), "fd 5");
        assert_eq!(arg(WireArg::Str(None)), "nil");
    }

    #[test]
    fn strings_cannot_break_the_line() {
        assert_eq!(arg(WireArg::Str(Some(c"a \"b\" c"))), "\"a \\\"b\\\" c\"");
        assert_eq!(arg(WireArg::Str(Some(c"a\\b"))), "\"a\\\\b\"");
        assert_eq!(
            arg(WireArg::Str(Some(c"x\n[12:00:00.000000]  -> wl_display#1.error()"))),
            "\"x\\x0a[12:00:00.000000]  -> wl_display#1.error()\""
        );
        assert_eq!(arg(WireArg::Str(Some(c"\x1b[2J"))), "\"\\x1b[2J\"");
        assert_eq!(arg(WireArg::Str(Some(c"caf\xc3\xa9"))), "\"caf\u{e9}\"");
        assert_eq!(arg(WireArg::Str(Some(c"\xff"))), "\"\u{fffd}\"");
    }

    /// Check and strip the timestamp
    pub(crate) fn untimed(line: &str) -> &str {
        let (ts, rest) = line.split_once("] ").expect("no timestamp");
        let ts = ts.strip_prefix('[').expect("no timestamp");
        let b = ts.as_bytes();
        assert_eq!(b.len(), 15, "timestamp {ts}");
        assert!(b[2] == b':' && b[5] == b':' && b[8] == b'.', "timestamp {ts}");
        rest
    }

    #[test]
    fn lines_as_wayland_debug_reads_them() {
        let args = [WireArg::NewId(Some("wl_callback"), 3)];
        let line = WireLine {
            conn: 2,
            sent: false,
            interface: "wl_display",
            id: 1,
            message: "sync",
            args: &args,
        };
        assert_eq!(untimed(&line.to_string()), "<2> wl_display#1.sync(new id wl_callback#3)");
        let args = [WireArg::Uint(15)];
        let line = WireLine {
            conn: 2,
            sent: true,
            interface: "wl_callback",
            id: 3,
            message: "done",
            args: &args,
        };
        assert_eq!(untimed(&line.to_string()), "<2>  -> wl_callback#3.done(15)");
    }
}
