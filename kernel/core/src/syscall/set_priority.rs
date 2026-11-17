// SPDX-License-Identifier: MPL-2.0

use super::SyscallReturn;
use crate::{
    prelude::*,
    process::{
        Process, ResourceType::RLIMIT_NICE, credentials::capabilities::CapSet,
        posix_thread::AsPosixThread,
    },
    sched::Nice,
    security::lsm::hooks as lsm_hooks,
    syscall::get_priority::{PriorityTarget, get_threads},
    thread::Thread,
};

pub(super) fn sys_set_priority(
    which: i32,
    who: u32,
    prio: i32,
    ctx: &Context,
) -> Result<SyscallReturn> {
    let prio_target = PriorityTarget::new(which, who, ctx)?;
    let new_nice = Nice::try_from(prio.clamp(-20, 19) as i8).unwrap();
    let threads = get_threads(prio_target)?;

    // Linux attempts every target, retaining any error even if other targets succeed.
    let mut error = None;
    for (thread, process) in &threads {
        if let Err(err) = set_thread_priority(thread, process, new_nice, ctx) {
            error = Some(err);
        }
    }
    if let Some(error) = error {
        return Err(error);
    }
    Ok(SyscallReturn::Return(0))
}

fn set_thread_priority(
    thread: &Thread,
    target_process: &Process,
    new_nice: Nice,
    ctx: &Context,
) -> Result<()> {
    let target = thread.as_posix_thread().unwrap();
    let caller_euid = ctx.posix_thread.credentials().euid();
    let same_owner = {
        let credentials = target.credentials();
        caller_euid == credentials.ruid() || caller_euid == credentials.euid()
    };
    let can_change_owner = lsm_hooks::on_capable(lsm_hooks::CapableContext::new(
        target_process.user_ns().lock().as_ref(),
        ctx.posix_thread,
        CapSet::SYS_NICE,
    ))
    .is_ok();
    if !same_owner && !can_change_owner {
        return_errno_with_message!(Errno::EPERM, "the target thread belongs to another user");
    }

    let limit = target_process
        .resource_limits()
        .get_rlimit(RLIMIT_NICE)
        .get_cur();
    let can_raise_priority = lsm_hooks::on_capable(lsm_hooks::CapableContext::new(
        ctx.process.user_ns().lock().as_ref(),
        ctx.posix_thread,
        CapSet::SYS_NICE,
    ))
    .is_ok();
    // Take sleeping locks before the scheduler's IRQ-disabled policy lock.
    thread.sched_attr().update_nice(new_nice, |old_nice| {
        check_nice_limit(old_nice, new_nice, limit, can_raise_priority)
    })
}

fn check_nice_limit(old: Nice, new: Nice, limit: u64, privileged: bool) -> Result<()> {
    // Linux encodes nice [-20, 19] as RLIMIT_NICE [40, 1]. Do not truncate the limit.
    let required_limit = (20 - i32::from(new.value().get())) as u64;
    if new < old && required_limit > limit && !privileged {
        return_errno_with_message!(Errno::EACCES, "raising priority exceeds the nice limit");
    }
    Ok(())
}

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn nice_limit_checks_only_priority_increases() {
        let zero = Nice::default();
        let high = Nice::try_from(-18).unwrap();
        assert_eq!(
            check_nice_limit(zero, high, 0, false).unwrap_err().error(),
            Errno::EACCES
        );
        assert!(check_nice_limit(zero, high, 38, false).is_ok());
        assert_eq!(
            check_nice_limit(zero, high, 37, false).unwrap_err().error(),
            Errno::EACCES
        );
        assert!(check_nice_limit(zero, high, 0, true).is_ok());
        assert!(check_nice_limit(high, zero, 0, false).is_ok());
        assert!(check_nice_limit(high, high, 0, false).is_ok());
        assert!(check_nice_limit(zero, Nice::MAX, 0, false).is_ok());
        assert!(check_nice_limit(zero, Nice::MIN, 40, false).is_ok());
        assert!(check_nice_limit(zero, Nice::MIN, u64::MAX, false).is_ok());
    }
}
