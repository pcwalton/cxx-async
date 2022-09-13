/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/src/execlet.rs
//
// Allows the Rust polling interface to drive C++ tasks to completion.
//
// This is needed by the Folly backend, to allow awaiting semifutures.

use crate::SafeUnwrap;
use once_cell::sync::OnceCell;
use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::Weak;
use std::task::Context;
use std::task::RawWaker;
use std::task::RawWakerVTable;
use std::task::Waker;
use std::thread;

// Allows the Rust polling interface to drive C++ tasks to completion.
//
// This is needed by the Folly backend, to allow awaiting semifutures.
#[derive(Clone)]
#[doc(hidden)]
pub struct Execlet(Arc<RustExeclet>);

struct WeakExeclet(Weak<RustExeclet>);

// The type that C++ sees for an execlet. This is opaque as far as C++ is concerned.
#[doc(hidden)]
pub struct RustExeclet(Mutex<ExecletImpl>);

impl Execlet {
    // Creates a new execlet with no waker and an empty runqueue.
    fn new() -> Execlet {
        Execlet(Arc::new(RustExeclet(Mutex::new(ExecletImpl {
            runqueue: VecDeque::new(),
            waker: None,
            running: false,
        }))))
    }

    fn downgrade(&self) -> WeakExeclet {
        WeakExeclet(Arc::downgrade(&self.0))
    }

    // Consumes the reference.
    unsafe fn from_raw(ptr: *const RustExeclet) -> Execlet {
        Execlet(Arc::from_raw(ptr))
    }

    // Bumps the reference count.
    pub(crate) unsafe fn from_raw_ref(ptr: *const RustExeclet) -> Execlet {
        let this = Execlet::from_raw(ptr);
        mem::forget(this.clone());
        this
    }

    // Runs all tasks in the runqueue to completion.
    pub(crate) fn run(&self, cx: &mut Context) {
        // Lock.
        let mut guard = self.0.0.lock().safe_unwrap();
        safe_debug_assert!(!guard.running);
        guard.running = true;

        // Run as many tasks as we have.
        guard.waker = Some((*cx.waker()).clone());
        while let Some(task) = guard.runqueue.pop_front() {
            // Drop the lock so that the task that we run can safely enqueue new tasks without
            // deadlocking.
            drop(guard);
            unsafe {
                task.run();
            }
            // Re-acquire the lock.
            guard = self.0.0.lock().safe_unwrap();
        }

        // Unlock.
        guard.running = false;
    }

    // Submits a task to this execlet.
    fn submit(&self, task: ExecletTask) {
        let mut this = self.0.0.lock().safe_unwrap();
        this.runqueue.push_back(task);
        if !this.running {
            if let Some(ref waker) = this.waker {
                // Avoid possible deadlocks.
                let waker = (*waker).clone();
                drop(this);
                waker.wake_by_ref();
            }
        }
    }
}

// Data for each execlet.
struct ExecletImpl {
    // Tasks waiting to run.
    runqueue: VecDeque<ExecletTask>,
    // A Rust Waker that will be informed when we need to wake up and run some tasks in our
    // runqueue or yield the result.
    waker: Option<Waker>,
    // True if we're running; false otherwise. This flag is necessary to avoid deadlocks resulting
    // from recursive invocations.
    running: bool,
}

// A continuation in an execlet's run queue.
struct ExecletTask {
    // A C++ stub that resumes this task.
    run: unsafe extern "C" fn(*mut u8),
    // The task data, passed to `run`.
    data: *mut u8,
}

impl ExecletTask {
    // Creates a new `ExecletTask`.
    fn new(run: unsafe extern "C" fn(*mut u8), data: *mut u8) -> Self {
        Self { run, data }
    }

    // Resumes the task.
    unsafe fn run(self) {
        (self.run)(self.data)
    }
}

unsafe impl Send for ExecletTask {}
unsafe impl Sync for ExecletTask {}

pub(crate) struct ExecletReaper {
    execlets: Mutex<ExecletReaperQueue>,
    cond: Condvar,
}

struct ExecletReaperQueue {
    new: Vec<WeakExeclet>,
    old: Vec<Execlet>,
}

impl ExecletReaper {
    pub(crate) fn get() -> Arc<ExecletReaper> {
        static INSTANCE: OnceCell<Arc<ExecletReaper>> = OnceCell::new();
        (*INSTANCE.get_or_init(|| {
            let reaper = Arc::new(ExecletReaper {
                execlets: Mutex::new(ExecletReaperQueue {
                    new: vec![],
                    old: vec![],
                }),
                cond: Condvar::new(),
            });
            let reaper_ = reaper.clone();
            thread::spawn(move || ExecletReaper::run(reaper_));
            reaper
        }))
        .clone()
    }

    pub(crate) fn add(&self, execlet: Execlet) {
        let mut execlets = self.execlets.lock().safe_unwrap();
        execlets.new.push(execlet.downgrade());

        // Go ahead and start running the execlet if the reaper is sleeping. This makes sure that
        // we properly handle the following sequence of events:
        //
        // 1. User code drops the `CxxAsyncReceiver` from a task T and `CxxAsyncReceiver::drop()`
        //    starts running, but doesn't get here yet.
        // 2. The wrapped C++ future resolves on some C++ executor on another thread and submits a
        //    task U to the execlet.
        // 3. The original Waker is invoked and the Rust executor makes a note to resume task T.
        //    Because T is already running and has dropped the receiver, this does *not* cause the
        //    execlet to run.
        // 4. We add the execlet to this reaper.
        //
        // In this case, we have to make sure that we run task U, even though an execlet reaper
        // waker was never invoked.
        self.cond.notify_all();
    }

    fn run(self: Arc<Self>) {
        loop {
            let mut execlets = self.execlets.lock().safe_unwrap();
            while execlets.is_empty() {
                execlets = self.cond.wait(execlets).safe_unwrap();
            }

            let execlets_to_process = execlets.take();
            for execlet in execlets_to_process {
                drop(execlets);
                unsafe {
                    execlet.run(&mut Context::from_waker(&Waker::from_raw(
                        ExecletReaperWaker {
                            execlet: execlet.clone(),
                        }
                        .into_raw(),
                    )));
                }
                execlets = self.execlets.lock().safe_unwrap();
                execlets.old.push(execlet);
            }
        }
    }

    fn wake(&self) {
        self.cond.notify_all();
    }
}

impl ExecletReaperQueue {
    fn is_empty(&self) -> bool {
        self.new.is_empty() && self.old.is_empty()
    }

    fn take(&mut self) -> Vec<Execlet> {
        let mut execlets = mem::take(&mut self.old);
        for execlet in mem::take(&mut self.new).into_iter() {
            if let Some(execlet) = execlet.0.upgrade() {
                execlets.push(Execlet(execlet));
            }
        }
        execlets
    }
}

#[derive(Clone)]
struct ExecletReaperWaker {
    execlet: Execlet,
}

impl ExecletReaperWaker {
    unsafe fn from_raw(data: *const ()) -> ExecletReaperWaker {
        ExecletReaperWaker {
            execlet: Execlet(Arc::from_raw(data as *const RustExeclet)),
        }
    }

    fn into_raw(self) -> RawWaker {
        RawWaker::new(
            Arc::into_raw(self.execlet.0) as *const (),
            &EXECLET_REAPER_WAKER_VTABLE,
        )
    }

    fn wake(self) {
        ExecletReaper::get().wake();
    }
}

unsafe fn execlet_reaper_waker_clone(data: *const ()) -> RawWaker {
    let execlet_reaper_waker = ExecletReaperWaker::from_raw(data);
    let waker = execlet_reaper_waker.clone().into_raw();
    mem::forget(execlet_reaper_waker);
    waker
}

unsafe fn execlet_reaper_waker_wake(data: *const ()) {
    ExecletReaperWaker::from_raw(data).wake()
}

unsafe fn execlet_reaper_waker_wake_by_ref(data: *const ()) {
    let waker = ExecletReaperWaker::from_raw(data);
    waker.clone().wake();
    mem::forget(waker);
}

unsafe fn execlet_reaper_waker_drop(data: *const ()) {
    drop(ExecletReaperWaker::from_raw(data))
}

static EXECLET_REAPER_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    execlet_reaper_waker_clone,
    execlet_reaper_waker_wake,
    execlet_reaper_waker_wake_by_ref,
    execlet_reaper_waker_drop,
);

// Execlet FFI

// C++ calls this to create a new execlet.
#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn cxxasync_execlet_create() -> *const RustExeclet {
    Arc::into_raw(Execlet::new().0)
}

// C++ calls this to decrement the reference count on an execlet and free it if the count hits zero.
#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn cxxasync_execlet_release(this: *mut RustExeclet) -> bool {
    let execlet = Execlet::from_raw(this);
    Arc::strong_count(&execlet.0) > 1 // Also destroys the execlet reference.
}

// C++ calls this to submit a task to the execlet. This internally bumps the reference count.
#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn cxxasync_execlet_submit(
    this: *mut RustExeclet,
    run: extern "C" fn(*mut u8),
    task_data: *mut u8,
) {
    Execlet::from_raw_ref(this).submit(ExecletTask::new(run, task_data))
}
