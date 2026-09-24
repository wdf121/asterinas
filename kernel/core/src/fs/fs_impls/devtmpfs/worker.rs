// SPDX-License-Identifier: MPL-2.0

//! Request queue and kernel thread for serialized devtmpfs tree operations.

use ostd::sync::{Waiter, Waker};
use spin::Once;

use super::{DevtmpfsHandle, DevtmpfsNode, tree};
use crate::{prelude::*, thread::kernel_thread::ThreadOptions};

/// Creates a device node through `devtmpfsd` and returns its ownership handle.
pub(crate) fn create_node(node: DevtmpfsNode) -> Result<DevtmpfsHandle> {
    match submit(Request::CreateNode(node))? {
        Response::Handle(handle) => Ok(handle),
        Response::Unit => unreachable!(),
    }
}

/// Creates a symbolic link through `devtmpfsd` and returns its ownership handle.
pub(crate) fn create_symlink(path: String, target: String) -> Result<DevtmpfsHandle> {
    match submit(Request::CreateSymlink { path, target })? {
        Response::Handle(handle) => Ok(handle),
        Response::Unit => unreachable!(),
    }
}

/// Validates that a handle still names its original devtmpfs inode.
pub(crate) fn validate(handle: &DevtmpfsHandle) -> Result<()> {
    match submit(Request::Validate(handle.clone()))? {
        Response::Unit => Ok(()),
        Response::Handle(_) => unreachable!(),
    }
}

/// Deletes a handle's node through `devtmpfsd` using inode identity.
pub(crate) fn delete(handle: DevtmpfsHandle) -> Result<()> {
    match submit(Request::Delete(handle))? {
        Response::Unit => Ok(()),
        Response::Handle(_) => unreachable!(),
    }
}

/// Renames a handle's node through `devtmpfsd` with no-replace semantics.
///
/// The returned handle records the destination path and replaces the source
/// handle only after the worker has completed the identity-checked move.
pub(crate) fn rename_no_replace(
    handle: DevtmpfsHandle,
    new_path: String,
) -> Result<DevtmpfsHandle> {
    match submit(Request::RenameNoReplace { handle, new_path })? {
        Response::Handle(handle) => Ok(handle),
        Response::Unit => unreachable!(),
    }
}

/// Retains the legacy best-effort device-node deletion API for ordinary callers.
pub(crate) fn delete_node(node: DevtmpfsNode) -> Result<()> {
    match submit(Request::DeleteNode(node))? {
        Response::Unit => Ok(()),
        Response::Handle(_) => unreachable!(),
    }
}

pub(super) fn init_in_first_kthread() {
    ThreadOptions::new(devtmpfsd).spawn();
}

fn submit(request: Request) -> Result<Response> {
    let (waiter, waker) = Waiter::new_pair();
    let request = Arc::new(PendingRequest::new(request, waker));

    REQUEST_QUEUE.requests.lock().push_back(request.clone());
    if let Some(waker) = REQUEST_QUEUE.waker.get() {
        waker.wake_up();
    }

    waiter.wait();
    request.result.lock().take().unwrap()
}

fn devtmpfsd() {
    let (waiter, waker) = Waiter::new_pair();
    REQUEST_QUEUE.waker.call_once(|| waker);

    loop {
        let request = REQUEST_QUEUE.requests.lock().pop_front();
        let Some(request) = request else {
            waiter.wait();
            continue;
        };

        let result = match &request.request {
            Request::CreateNode(node) => tree::create_node(node).map(Response::Handle),
            Request::CreateSymlink { path, target } => {
                tree::create_symlink(path, target).map(Response::Handle)
            }
            Request::Validate(handle) => tree::validate(handle).map(|()| Response::Unit),
            Request::Delete(handle) => tree::delete(handle.clone()).map(|()| Response::Unit),
            Request::RenameNoReplace { handle, new_path } => {
                let mut handle = handle.clone();
                tree::rename_no_replace(&mut handle, new_path).map(|()| Response::Handle(handle))
            }
            Request::DeleteNode(node) => tree::delete_node(node).map(|()| Response::Unit),
        };
        *request.result.lock() = Some(result);
        request.waker.wake_up();
    }
}

struct RequestQueue {
    requests: SpinLock<VecDeque<Arc<PendingRequest>>>,
    waker: Once<Arc<Waker>>,
}

static REQUEST_QUEUE: RequestQueue = RequestQueue {
    requests: SpinLock::new(VecDeque::new()),
    waker: Once::new(),
};

struct PendingRequest {
    request: Request,
    result: Mutex<Option<Result<Response>>>,
    waker: Arc<Waker>,
}

impl PendingRequest {
    fn new(request: Request, waker: Arc<Waker>) -> Self {
        Self {
            request,
            result: Mutex::new(None),
            waker,
        }
    }
}

enum Request {
    CreateNode(DevtmpfsNode),
    CreateSymlink {
        path: String,
        target: String,
    },
    Validate(DevtmpfsHandle),
    Delete(DevtmpfsHandle),
    RenameNoReplace {
        handle: DevtmpfsHandle,
        new_path: String,
    },
    DeleteNode(DevtmpfsNode),
}

enum Response {
    Unit,
    Handle(DevtmpfsHandle),
}

#[cfg(ktest)]
pub(super) fn init_for_ktest() {
    static START: Once<()> = Once::new();

    crate::time::clocks::init_for_ktest();
    START.call_once(|| {
        ThreadOptions::new(devtmpfsd).spawn();
    });
}
