use std::time::{Duration, Instant};

/// Coalesce allocations/releases without polling or trimming on every frame.
#[derive(Default)]
pub struct TrimSchedule {
    pending: bool,
    last_trim: Option<Instant>,
}

impl TrimSchedule {
    pub fn request(&mut self, now: Instant) -> Option<Duration> {
        if self.pending {
            return None;
        }
        self.pending = true;
        let cooldown = self.last_trim.map_or(Duration::ZERO, |last| {
            (last + Duration::from_secs(30)).saturating_duration_since(now)
        });
        Some(cooldown.max(Duration::from_secs(3)))
    }

    pub fn completed(&mut self, now: Instant) {
        self.pending = false;
        self.last_trim = Some(now);
    }

    pub fn cancel(&mut self) {
        self.pending = false;
    }
}

pub fn reclaim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: glibc's thread-safe allocator operation takes no pointers and
        // only returns pages no longer occupied by live allocations.
        unsafe { malloc_trim(0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn reclamation_coalesces_bursts_and_has_no_idle_timer() {
        let now = Instant::now();
        let mut schedule = TrimSchedule::default();
        assert_eq!(schedule.request(now), Some(Duration::from_secs(3)));
        assert_eq!(schedule.request(now + Duration::from_secs(1)), None);
        schedule.completed(now + Duration::from_secs(3));
        assert_eq!(
            schedule.request(now + Duration::from_secs(4)),
            Some(Duration::from_secs(29))
        );
        assert_eq!(schedule.request(now + Duration::from_secs(5)), None);
        schedule.completed(now + Duration::from_secs(33));
        assert_eq!(
            schedule.request(now + Duration::from_secs(90)),
            Some(Duration::from_secs(3))
        );
    }
}
