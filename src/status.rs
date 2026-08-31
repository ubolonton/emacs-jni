use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    NotStarted = 0,
    Starting = 1,
    Ready = 2,
    Pausing = 3,
    Paused = 4,
    Stopping = 5,
    Stopped = 6,
    StartFailed = 7,
    StopFailed = 8,
    Crashed = 9,
}

impl Status {
    fn from_u8(v: u8) -> Status {
        match v {
            0 => Status::NotStarted,
            1 => Status::Starting,
            2 => Status::Ready,
            3 => Status::Pausing,
            4 => Status::Paused,
            5 => Status::Stopping,
            6 => Status::Stopped,
            7 => Status::StartFailed,
            8 => Status::StopFailed,
            9 => Status::Crashed,
            _ => unreachable!("invalid Status discriminant"),
        }
    }

    pub fn as_tag(self) -> &'static str {
        match self {
            Status::NotStarted => "not-started",
            Status::Starting => "starting",
            Status::Ready => "ready",
            Status::Pausing => "pausing",
            Status::Paused => "paused",
            Status::Stopping => "stopping",
            Status::Stopped => "stopped",
            Status::StartFailed => "start-failed",
            Status::StopFailed => "stop-failed",
            Status::Crashed => "crashed",
        }
    }
}

pub struct StatusCell(AtomicU8);

impl StatusCell {
    pub const fn new() -> Self {
        StatusCell(AtomicU8::new(Status::NotStarted as u8))
    }

    pub fn get(&self) -> Status {
        Status::from_u8(self.0.load(Ordering::SeqCst))
    }

    fn set(&self, s: Status) {
        self.0.store(s as u8, Ordering::SeqCst);
    }

    fn try_transition(&self, allowed_from: &[Status], to: Status) -> Result<(), Status> {
        let current = self.get();
        if allowed_from.contains(&current) {
            self.set(to);
            Ok(())
        } else {
            Err(current)
        }
    }

    /// Legal from not-started, paused, stopped, start-failed, stop-failed,
    /// or crashed. Covers both a first-time start and a resume-from-pause;
    /// the caller (see worker.rs in Task 4) decides which by checking
    /// whether a worker thread is already running, independent of this
    /// state check.
    pub fn begin_start(&self) -> Result<(), Status> {
        self.try_transition(
            &[
                Status::NotStarted,
                Status::Paused,
                Status::Stopped,
                Status::StartFailed,
                Status::StopFailed,
                Status::Crashed,
            ],
            Status::Starting,
        )
    }

    pub fn begin_pause(&self) -> Result<(), Status> {
        self.try_transition(&[Status::Ready], Status::Pausing)
    }

    pub fn begin_stop(&self) -> Result<(), Status> {
        self.try_transition(&[Status::Ready, Status::Paused], Status::Stopping)
    }

    pub fn finish(&self, to: Status) {
        self.set(to);
    }
}

pub static STATUS: StatusCell = StatusCell::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_from_not_started_succeeds() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_start(), Ok(()));
        assert_eq!(cell.get(), Status::Starting);
    }

    #[test]
    fn start_from_paused_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Paused);
        assert_eq!(cell.begin_start(), Ok(()));
        assert_eq!(cell.get(), Status::Starting);
    }

    #[test]
    fn start_from_ready_fails() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_start(), Err(Status::Ready));
        assert_eq!(cell.get(), Status::Ready);
    }

    #[test]
    fn pause_from_ready_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_pause(), Ok(()));
        assert_eq!(cell.get(), Status::Pausing);
    }

    #[test]
    fn pause_from_not_started_fails() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_pause(), Err(Status::NotStarted));
    }

    #[test]
    fn stop_from_ready_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_stop(), Ok(()));
        assert_eq!(cell.get(), Status::Stopping);
    }

    #[test]
    fn stop_from_paused_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Paused);
        assert_eq!(cell.begin_stop(), Ok(()));
        assert_eq!(cell.get(), Status::Stopping);
    }

    #[test]
    fn stop_from_not_started_fails() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_stop(), Err(Status::NotStarted));
    }

    #[test]
    fn as_tag_matches_spec_names() {
        assert_eq!(Status::NotStarted.as_tag(), "not-started");
        assert_eq!(Status::StartFailed.as_tag(), "start-failed");
        assert_eq!(Status::StopFailed.as_tag(), "stop-failed");
        assert_eq!(Status::Crashed.as_tag(), "crashed");
    }
}
