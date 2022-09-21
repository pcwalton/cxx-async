/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/include/rust/cxx_async_folly.h

#ifndef RUST_CXX_ASYNC_FOLLY_H
#define RUST_CXX_ASYNC_FOLLY_H

#include <folly/Executor.h>
#include <folly/Try.h>
#include <folly/executors/ManualExecutor.h>
#include <folly/experimental/coro/Task.h>
#include <folly/experimental/coro/ViaIfAsync.h>
#include <atomic>
#include <mutex>
#include <queue>
#include <type_traits>
#include "rust/cxx_async.h"

namespace rust {
namespace async {

// Callback that Rust uses to start a C++ task.
extern "C" inline void execlet_run_task(void* task_ptr) {
  folly::Function<void()>* task =
      reinterpret_cast<folly::Function<void()>*>(task_ptr);
  (*task)();
  delete task;
}

// Folly-specific interface to execlets.
class FollyExeclet : public folly::Executor {
  Execlet& m_rust_execlet;

  // NB: This starts out at *zero*, not at one. Folly is weird in that it
  // expects the object to be destroyed once `keepAliveRelease()` is called a
  // number of times greater than zero and equal to the number of times
  // `keepAliveAcquire()` was called.
  std::atomic<uintptr_t> m_refcount;

  FollyExeclet(const FollyExeclet&) = delete;
  FollyExeclet& operator=(const FollyExeclet&) = delete;

 public:
  FollyExeclet(Execlet& rust_execlet) : m_rust_execlet(rust_execlet) {}

  Execlet& rust_execlet() {
    return m_rust_execlet;
  }

  // Submits a task to the execlet.
  virtual void add(folly::Func task) {
    m_rust_execlet.submit(new folly::Func(std::move(task)), execlet_run_task);
  }

  virtual bool keepAliveAcquire() noexcept {
    m_refcount.fetch_add(1);
    return true;
  }

  virtual void keepAliveRelease() noexcept {
    // Decrement the reference count and destroys this wrapper if the execlet is
    // now dead.
    uintptr_t last_refcount = m_refcount.fetch_sub(1);
    CXXASYNC_ASSERT(last_refcount > 0);
    if (last_refcount == 1) {
      delete this;
    }
  }
};

// Allows Folly semi-awaitables (including Folly tasks) to be awaited.
template <typename SemiAwaitable, typename Future>
class AwaitTransformer<
    SemiAwaitable,
    Future,
    std::void_t<folly::coro::semi_await_result_t<SemiAwaitable>()>> {
  AwaitTransformer() = delete;

 public:
  static auto await_transform(
      RustPromiseBase<Future>& promise,
      SemiAwaitable&& semiawaitable) noexcept {
    return std::move(folly::coro::co_viaIfAsync(
        new FollyExeclet(promise.execlet()),
        std::forward<SemiAwaitable>(semiawaitable)));
  }
};

} // namespace async
} // namespace rust

#endif // RUST_CXX_ASYNC_FOLLY_H
