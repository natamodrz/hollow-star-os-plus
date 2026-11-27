//! BeOS-inspired priority levels (`1..=120`) for Delta Scheduler.

/// Scheduling priority (`1..=120`), matching BeOS interactive and Real-Time tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Priority(u8);

impl Priority {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 120;

    /// Per-CPU idle threads (`1`).
    pub const IDLE: Self = Self(1);
    /// Low-priority background tasks (`5`).
    pub const LOW: Self = Self(5);
    /// Normal interactive CLI/user processes (`10`).
    pub const NORMAL: Self = Self(10);
    /// Display/UI compositor and input threads (`15`).
    pub const DISPLAY: Self = Self(15);
    /// Urgent system services (`20`).
    pub const URGENT: Self = Self(20);
    /// Minimum Real-Time audio/video priority (`100`).
    pub const REALTIME_MIN: Self = Self(100);
    /// Maximum Real-Time audio/video priority (`120`).
    pub const REALTIME_MAX: Self = Self(120);

    /// Creates a [`Priority`], clamping `raw` into `1..=120`.
    pub const fn new(raw: u8) -> Self {
        if raw < Self::MIN {
            Self(Self::MIN)
        } else if raw > Self::MAX {
            Self(Self::MAX)
        } else {
            Self(raw)
        }
    }

    /// Returns the numeric priority value (`1..=120`).
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    /// Returns `true` if this priority belongs to the Real-Time tier (`100..=120`).
    pub const fn is_realtime(self) -> bool {
        self.0 >= Self::REALTIME_MIN.0
    }

    /// Returns the default timer-tick quantum for this priority level.
    pub const fn default_quantum(self) -> u32 {
        if self.is_realtime() {
            // RT threads run until they block, yield, or a higher-priority RT thread wakes.
            u32::MAX
        } else if self.0 >= Self::DISPLAY.0 {
            2 // 20 ms short quantum for responsive UI/urgent services
        } else {
            4 // 40 ms quantum for normal/background threads
        }
    }
}
