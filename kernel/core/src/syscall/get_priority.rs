// SPDX-License-Identifier: MPL-2.0

use super::SyscallReturn;
use crate::{
    prelude::*,
    process::{Pgid, Process, Uid, pid_table, posix_thread::AsPosixThread},
    sched::Nice,
    thread::{Thread, Tid},
};

pub(super) fn sys_get_priority(which: i32, who: u32, ctx: &Context) -> Result<SyscallReturn> {
    let prio_target = PriorityTarget::new(which, who, ctx)?;
    debug!("get_priority prio_target: {:?}", prio_target);

    let threads = get_threads(prio_target)?;
    let highest_prio = {
        let mut nice = Nice::MAX.value().get();
        for (thread, _) in threads.iter() {
            let thread_nice = thread.sched_attr().nice().value().get();
            // Returns the highest priority enjoyed by the threads
            if thread_nice < nice {
                nice = thread_nice;
            }
        }

        // The system call returns nice values translated to the range 40 to 1,
        // since a negative return value would be interpreted as an error.
        20 - nice
    };

    Ok(SyscallReturn::Return(highest_prio as _))
}

pub(super) fn get_threads(prio_target: PriorityTarget) -> Result<Vec<(Arc<Thread>, Arc<Process>)>> {
    // Keep each process alive too: POSIX threads hold only a weak owner reference.
    // Lock order: PID table -> group of process (as in setpgid/setsid).
    let threads = {
        let table = pid_table::pid_table_mut();
        match prio_target {
            PriorityTarget::Thread(tid) => {
                let thread = table.get_thread(tid).ok_or(Error::new(Errno::ESRCH))?;
                let process = thread.as_posix_thread().unwrap().process();
                vec![(thread, process)]
            }
            PriorityTarget::ProcessGroup(pgid) => table
                .iter_threads()
                .filter_map(|thread| {
                    let process = thread.as_posix_thread().unwrap().process();
                    (process.pgid() == pgid).then_some((thread, process))
                })
                .collect(),
            PriorityTarget::User(uid) => table
                .iter_threads()
                .filter(|thread| uid == thread.as_posix_thread().unwrap().credentials().ruid())
                .map(|thread| {
                    let process = thread.as_posix_thread().unwrap().process();
                    (thread, process)
                })
                .collect(),
        }
    };
    if threads.is_empty() {
        return_errno!(Errno::ESRCH);
    }
    Ok(threads)
}

#[derive(Debug)]
pub(super) enum PriorityTarget {
    Thread(Tid),
    ProcessGroup(Pgid),
    User(Uid),
}

impl PriorityTarget {
    pub(super) fn new(which: i32, who: u32, ctx: &Context) -> Result<Self> {
        let which = Which::try_from(which)
            .map_err(|_| Error::with_message(Errno::EINVAL, "invalid which value"))?;
        Ok(match which {
            Which::PRIO_PROCESS => {
                let tid = if who == 0 {
                    ctx.posix_thread.tid()
                } else {
                    who as Tid
                };
                Self::Thread(tid)
            }
            Which::PRIO_PGRP => {
                let pgid = if who == 0 {
                    ctx.process.pgid()
                } else {
                    who as Pgid
                };
                Self::ProcessGroup(pgid)
            }
            Which::PRIO_USER => {
                let uid = if who == 0 {
                    ctx.posix_thread.credentials().ruid()
                } else {
                    Uid::new(who)
                };
                Self::User(uid)
            }
        })
    }
}

#[expect(non_camel_case_types)]
#[repr(i32)]
#[derive(Clone, Debug, TryFromInt)]
pub(super) enum Which {
    PRIO_PROCESS = 0,
    PRIO_PGRP = 1,
    PRIO_USER = 2,
}
