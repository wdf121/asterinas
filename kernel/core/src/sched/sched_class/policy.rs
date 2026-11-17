// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicU8, Ordering::Relaxed};

use atomic_integer_wrapper::define_atomic_version_of_integer_like_type;
use int_to_c_enum::TryFromInt;
use ostd::sync::SpinLock;

pub(crate) use super::real_time::{RealTimePolicy, RealTimePriority};
use crate::sched::nice::Nice;

/// The User-chosen scheduling policy.
///
/// The scheduling policies are specified by the user, usually through its priority.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum SchedPolicy {
    #[expect(dead_code)]
    Stop,
    RealTime {
        rt_prio: RealTimePriority,
        rt_policy: RealTimePolicy,
    },
    Fair(Nice),
    // User-visible SCHED_IDLE; distinct from the kernel-only idle class.
    FairIdle,
    Idle,
}

impl Default for SchedPolicy {
    fn default() -> Self {
        Self::Fair(Nice::default())
    }
}

/// The Linux scheduling policy code.
///
/// Reference: <https://elixir.bootlin.com/linux/v6.17.7/source/include/uapi/linux/sched.h#L112>.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, TryFromInt)]
pub(crate) enum LinuxSchedPolicy {
    Normal = 0,
    Fifo = 1,
    RoundRobin = 2,
    Batch = 3, // Not supported.
    Iso = 4,   // Reserved but not implemented yet on Linux.
    Idle = 5,
    Deadline = 6, // Not supported.
    Ext = 7,      // Not supported.
}

/// Projects internal scheduling policies onto Linux's user-visible policy codes.
///
/// This projection is many-to-one: `Stop` is reported as `Fifo`, and the
/// kernel-only `Idle` policy is reported as `Normal`.
impl From<SchedPolicy> for LinuxSchedPolicy {
    fn from(policy: SchedPolicy) -> Self {
        match policy {
            SchedPolicy::Stop => LinuxSchedPolicy::Fifo,
            SchedPolicy::RealTime {
                rt_policy: RealTimePolicy::Fifo,
                ..
            } => LinuxSchedPolicy::Fifo,
            SchedPolicy::RealTime {
                rt_policy: RealTimePolicy::RoundRobin { .. },
                ..
            } => LinuxSchedPolicy::RoundRobin,
            SchedPolicy::Fair(_) | SchedPolicy::Idle => LinuxSchedPolicy::Normal,
            SchedPolicy::FairIdle => LinuxSchedPolicy::Idle,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, TryFromInt)]
pub(super) enum SchedPolicyKind {
    Stop = 0,
    RealTime = 1,
    Fair = 2,
    Idle = 3,
}

impl SchedPolicy {
    pub(super) fn kind(&self) -> SchedPolicyKind {
        match self {
            SchedPolicy::Stop => SchedPolicyKind::Stop,
            SchedPolicy::RealTime { .. } => SchedPolicyKind::RealTime,
            SchedPolicy::Fair(_) | SchedPolicy::FairIdle => SchedPolicyKind::Fair,
            SchedPolicy::Idle => SchedPolicyKind::Idle,
        }
    }
}

define_atomic_version_of_integer_like_type!(SchedPolicyKind, try_from = true, {
    #[derive(Debug)]
    pub(crate) struct AtomicSchedPolicyKind(AtomicU8);
});

impl From<SchedPolicyKind> for u8 {
    fn from(value: SchedPolicyKind) -> Self {
        value as _
    }
}

#[derive(Debug)]
pub(super) struct SchedPolicyState {
    kind: AtomicSchedPolicyKind,
    params: SpinLock<SchedParams>,
}

enum NiceUpdate {
    FromPolicy,
    Preserve,
}

#[derive(Debug)]
struct SchedParams {
    policy: SchedPolicy,
    // Linux retains nice even while an RT or SCHED_IDLE policy is active.
    nice: Nice,
}

impl SchedPolicyState {
    pub(crate) fn new(policy: SchedPolicy, nice: Nice) -> Self {
        Self {
            kind: AtomicSchedPolicyKind::new(policy.kind()),
            params: SpinLock::new(SchedParams { policy, nice }),
        }
    }

    pub(crate) fn kind(&self) -> SchedPolicyKind {
        self.kind.load(Relaxed)
    }

    pub(crate) fn get(&self) -> SchedPolicy {
        self.params.disable_irq().lock().policy
    }

    pub(crate) fn set(&self, policy: SchedPolicy, update: impl FnOnce(SchedPolicy)) {
        self.set_with(policy, update, NiceUpdate::FromPolicy);
    }

    pub(crate) fn set_preserving_nice(
        &self,
        policy: SchedPolicy,
        update: impl FnOnce(SchedPolicy),
    ) {
        self.set_with(policy, update, NiceUpdate::Preserve);
    }

    fn set_with(
        &self,
        mut policy: SchedPolicy,
        update: impl FnOnce(SchedPolicy),
        nice_update: NiceUpdate,
    ) {
        let mut this = self.params.disable_irq().lock();
        if matches!(nice_update, NiceUpdate::Preserve) && matches!(policy, SchedPolicy::Fair(_)) {
            policy = SchedPolicy::Fair(this.nice);
        }

        // Keep the old base slice factor if the new policy doesn't specify one.
        if let (
            SchedPolicy::RealTime {
                rt_policy:
                    RealTimePolicy::RoundRobin {
                        base_slice_factor: slot,
                    },
                ..
            },
            SchedPolicy::RealTime {
                rt_policy: RealTimePolicy::RoundRobin { base_slice_factor },
                ..
            },
        ) = (this.policy, &mut policy)
        {
            *base_slice_factor = slot.or(*base_slice_factor);
        }

        update(policy);
        self.kind.store(policy.kind(), Relaxed);
        if let SchedPolicy::Fair(nice) = policy {
            this.nice = nice;
        }
        this.policy = policy;
    }

    pub(crate) fn update<T>(&self, update: impl FnOnce(&mut SchedPolicy) -> T) -> T {
        let mut params = self.params.disable_irq().lock();
        let result = update(&mut params.policy);
        if let SchedPolicy::Fair(nice) = params.policy {
            params.nice = nice;
        }
        self.kind.store(params.policy.kind(), Relaxed);
        result
    }

    pub(crate) fn snapshot(&self) -> (SchedPolicy, Nice) {
        let params = self.params.disable_irq().lock();
        (params.policy, params.nice)
    }

    pub(crate) fn update_nice<E>(
        &self,
        nice: Nice,
        check: impl FnOnce(Nice) -> Result<(), E>,
        update_fair: impl FnOnce(Nice),
    ) -> Result<(), E> {
        let mut params = self.params.disable_irq().lock();
        check(params.nice)?;
        params.nice = nice;
        if let SchedPolicy::Fair(old_nice) = &mut params.policy {
            *old_nice = nice;
            update_fair(nice);
        }
        Ok(())
    }
}
