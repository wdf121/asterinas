// SPDX-License-Identifier: MPL-2.0

//! Completion-owned table references are released only in task context.

use alloc::{boxed::Box, sync::Arc};
use core::ops::Deref;

use ostd::sync::{LocalIrqDisabled, SpinLock, WaitQueue};

use crate::DmTable;

static PENDING: SpinLock<Option<Box<Node>>, LocalIrqDisabled> = SpinLock::new(None);
static READY: WaitQueue = WaitQueue::new();

struct Node {
    table: Arc<DmTable>,
    next: Option<Box<Node>>,
}

/// Allocates queue storage at submission, never in the completion callback.
pub(super) struct CompletionTable(Option<Box<Node>>);

impl CompletionTable {
    pub(super) fn new(table: &Arc<DmTable>) -> Self {
        Self(Some(Box::new(Node {
            table: table.clone(),
            next: None,
        })))
    }
}

impl Deref for CompletionTable {
    type Target = DmTable;

    fn deref(&self) -> &Self::Target {
        &self.0.as_ref().unwrap().table
    }
}

impl Drop for CompletionTable {
    fn drop(&mut self) {
        let mut node = self.0.take().unwrap();
        {
            let mut pending = PENDING.lock();
            node.next = pending.take();
            *pending = Some(node);
        }
        READY.wake_one();
    }
}

fn take_pending() -> Option<Box<Node>> {
    PENDING.lock().take()
}

fn reclaim(mut pending: Option<Box<Node>>) {
    // Detach each link before dropping: neither recursive destruction nor target
    // destructors may run with IRQs disabled or with the queue lock held.
    while let Some(mut node) = pending {
        pending = node.next.take();
        drop(node);
    }
}

/// Runs the permanent DM reclamation worker.
///
/// The kernel must start this on a sleepable kernel thread before exposing DM.
/// BIO completion accounting does not wait for this worker; backing leases remain
/// held until reclamation, so unregistration may temporarily still report busy.
pub fn run_table_reclaimer() -> ! {
    loop {
        let pending = READY.wait_until(take_pending);
        reclaim(Some(pending));
    }
}

#[cfg(ktest)]
pub(crate) fn reclaim_for_test() {
    reclaim(take_pending());
}
