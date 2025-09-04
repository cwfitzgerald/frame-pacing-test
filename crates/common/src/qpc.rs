use std::time::Duration;

use once_cell::sync::Lazy;

struct QpcTimer {
    qpc_frequency: u64,
}

impl QpcTimer {
    fn new() -> Self {
        let mut qpc_frequency = 0;
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceFrequency(&mut qpc_frequency)
                .unwrap();
        }
        Self { qpc_frequency: qpc_frequency as u64 }
    }

    fn now(&self) -> u128 {
        let mut qpc_time = 0;
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceCounter(&mut qpc_time).unwrap();
        }
        self.from_raw(qpc_time)
    }

    fn from_raw(&self, qpc_time: i64) -> u128 {
        (qpc_time as u128 * 1_000_000_000) / self.qpc_frequency as u128
    }
}

static QPC_TIME: Lazy<QpcTimer> = Lazy::new(QpcTimer::new);

#[derive(Debug, Clone, Copy)]
pub struct QpcInstant {
    /// Raw timestamp in nanoseconds
    time: u128,
}

impl QpcInstant {
    pub fn now() -> Self {
        Self { time: QPC_TIME.now() }
    }

    pub fn from_raw(qpc_time: i64) -> Self {
        Self { time: QPC_TIME.from_raw(qpc_time) }
    }

    pub fn elapsed(&self) -> Duration {
        let now = Self::now();
        now - *self
    }
}

impl std::ops::Add<Duration> for QpcInstant {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        Self { time: self.time + rhs.as_nanos() }
    }
}

impl std::ops::Sub for QpcInstant {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        let elapsed = self.time.saturating_sub(rhs.time);
        Duration::new((elapsed / 1_000_000_000) as u64, (elapsed % 1_000_000_000) as u32)
    }
}
