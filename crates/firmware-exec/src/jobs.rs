//! Allocation-free admission and cancellation for a single keyboard executor.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobHandle(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Running,
    Releasing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job<O> {
    pub handle: JobHandle,
    pub owner: O,
    pub phase: Phase,
    pub cancelled: bool,
}

pub struct Controller<O> {
    next: u64,
    active: Option<Job<O>>,
    result_claimed: bool,
}

impl<O: Copy> Default for Controller<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: Copy> Controller<O> {
    pub const fn new() -> Self {
        Self {
            next: 0,
            active: None,
            result_claimed: false,
        }
    }

    /// Reservation includes queued work; a second producer never preempts it.
    pub fn reserve(&mut self, owner: O) -> Option<JobHandle> {
        if self.active.is_some() {
            return None;
        }
        // Never reuse a handle, including after integer exhaustion.
        self.next = self.next.checked_add(1)?;
        let handle = JobHandle(self.next);
        self.active = Some(Job {
            handle,
            owner,
            phase: Phase::Queued,
            cancelled: false,
        });
        self.result_claimed = false;
        Some(handle)
    }

    pub fn active(&self) -> Option<Job<O>> {
        self.active
    }

    pub fn start(&mut self, handle: JobHandle) -> bool {
        match self.active.as_mut() {
            Some(job) if job.handle == handle && job.phase == Phase::Queued => {
                job.phase = Phase::Running;
                true
            }
            _ => false,
        }
    }

    /// Cancellation is sticky and scoped to the currently accepted handle.
    pub fn cancel(&mut self, handle: JobHandle) -> bool {
        match self.active.as_mut() {
            Some(job) if job.handle == handle && job.phase != Phase::Releasing => {
                job.cancelled = true;
                true
            }
            _ => false,
        }
    }

    /// Resolve ownership and mark cancellation in one synchronous operation.
    pub fn cancel_matching(&mut self, matches: impl FnOnce(O) -> bool) -> bool {
        match self.active {
            Some(job) if matches(job.owner) => self.cancel(job.handle),
            _ => false,
        }
    }

    /// Commit the terminal decision before cleanup. Later Stop requests are no-ops.
    pub fn begin_release(&mut self, handle: JobHandle) -> Option<bool> {
        let job = self.active.as_mut()?;
        if job.handle != handle || job.phase == Phase::Releasing {
            return None;
        }
        job.phase = Phase::Releasing;
        Some(job.cancelled)
    }

    /// Claim completion once, including when the USB task is dropped during delivery.
    pub fn claim_result(&mut self, handle: JobHandle) -> Option<Job<O>> {
        let job = self.active?;
        if job.handle != handle || job.phase != Phase::Releasing || self.result_claimed {
            return None;
        }
        self.result_claimed = true;
        Some(job)
    }

    /// Admission stays closed until the caller has completed bounded cleanup.
    pub fn finish(&mut self, handle: JobHandle) -> Option<Job<O>> {
        match self.active {
            Some(job) if job.handle == handle && job.phase == Phase::Releasing => {
                self.active = None;
                Some(job)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn competing_producers_never_preempt_queued_running_or_releasing_work() {
        let mut jobs = Controller::new();
        let local = jobs.reserve("local").unwrap();
        assert!(jobs.reserve("browser").is_none());
        assert!(jobs.start(local));
        assert!(jobs.reserve("browser").is_none());
        assert_eq!(jobs.begin_release(local), Some(false));
        assert!(jobs.reserve("browser").is_none());
        assert_eq!(jobs.finish(local).unwrap().owner, "local");
        assert!(jobs.reserve("browser").is_some());
    }

    #[test]
    fn queued_stop_is_sticky_and_stale_handles_cannot_cancel_the_next_job() {
        let mut jobs = Controller::new();
        let old = jobs.reserve(1).unwrap();
        assert!(jobs.cancel(old));
        assert!(jobs.cancel(old));
        assert!(jobs.start(old));
        assert_eq!(jobs.begin_release(old), Some(true));
        jobs.finish(old).unwrap();
        let new = jobs.reserve(2).unwrap();
        assert_ne!(new, old);
        assert!(!jobs.cancel(old));
        assert!(!jobs.start(old));
        assert!(jobs.finish(old).is_none());
        assert!(!jobs.active().unwrap().cancelled);
    }

    #[test]
    fn completion_and_stop_have_a_single_terminal_decision() {
        let mut jobs = Controller::new();
        let handle = jobs.reserve(1).unwrap();
        jobs.start(handle);
        assert!(jobs.finish(handle).is_none());
        assert_eq!(jobs.begin_release(handle), Some(false));
        assert!(!jobs.cancel(handle));
        assert_eq!(jobs.begin_release(handle), None);
        assert!(jobs.finish(handle).is_some());
        assert!(jobs.finish(handle).is_none());
    }

    #[test]
    fn running_stop_wins_before_the_terminal_decision() {
        let mut jobs = Controller::new();
        let handle = jobs.reserve(1).unwrap();
        jobs.start(handle);
        assert!(jobs.cancel(handle));
        assert_eq!(jobs.begin_release(handle), Some(true));
        assert!(jobs.finish(handle).unwrap().cancelled);
    }

    #[test]
    fn old_browser_sessions_and_effects_cannot_cancel_local_or_replacement_jobs() {
        #[derive(Clone, Copy)]
        enum Owner {
            Local,
            Browser { session: u32, effect: u64 },
        }
        let mut jobs = Controller::new();
        let local = jobs.reserve(Owner::Local).unwrap();
        let old_browser = |owner| {
            matches!(
                owner,
                Owner::Browser {
                    session: 1,
                    effect: 7
                }
            )
        };
        assert!(!jobs.cancel_matching(old_browser));
        jobs.begin_release(local);
        jobs.finish(local);
        let browser = jobs
            .reserve(Owner::Browser {
                session: 2,
                effect: 7,
            })
            .unwrap();
        assert!(!jobs.cancel_matching(old_browser));
        assert!(!jobs.cancel_matching(|owner| matches!(
            owner,
            Owner::Browser {
                session: 2,
                effect: 6
            }
        )));
        assert!(jobs.cancel_matching(|owner| matches!(
            owner,
            Owner::Browser {
                session: 2,
                effect: 7
            }
        )));
        assert_eq!(jobs.begin_release(browser), Some(true));
    }

    #[test]
    fn usb_shutdown_cannot_duplicate_an_already_claimed_completion() {
        let mut jobs = Controller::new();
        let handle = jobs.reserve("browser").unwrap();
        assert!(jobs.claim_result(handle).is_none());
        jobs.start(handle);
        jobs.begin_release(handle);
        assert!(jobs.claim_result(handle).is_some());
        // USB task destruction attempts release/completion while the router is blocked.
        assert!(jobs.begin_release(handle).is_none());
        assert!(jobs.claim_result(handle).is_none());
        assert!(jobs.reserve("local").is_none());
        jobs.finish(handle).unwrap();
        let next = jobs.reserve("local").unwrap();
        jobs.begin_release(next);
        assert!(jobs.claim_result(next).is_some());
    }
}
