// cxx-async/include/rust/cxx_async_folly.h

#ifndef RUST_CXX_ASYNC_FOLLY_H
#define RUST_CXX_ASYNC_FOLLY_H

#include <folly/Executor.h>
#include <folly/Try.h>
#include <folly/executors/ManualExecutor.h>
#include <folly/experimental/coro/Task.h>
#include <folly/experimental/coro/ViaIfAsync.h>
#include <mutex>
#include <queue>
#include "rust/cxx_async.h"

namespace rust {
namespace async {

// Callback that Rust uses to start a C++ task.
extern "C" inline void execlet_run_task(void* task_ptr) {
    folly::Function<void()>* task = reinterpret_cast<folly::Function<void()>*>(task_ptr);
    (*task)();
    delete task;
}

// Folly-specific interface to execlets.
class FollyExeclet : public folly::Executor {
    Execlet& m_rust_execlet;

    FollyExeclet(const FollyExeclet&) = delete;
    FollyExeclet& operator=(const FollyExeclet&) = delete;

   public:
    FollyExeclet(Execlet& rust_execlet) : m_rust_execlet(rust_execlet) {}
    Execlet& rust_execlet() { return m_rust_execlet; }

    // Submits a task to the execlet.
    virtual void add(folly::Func task) {
        m_rust_execlet.submit(new folly::Func(std::move(task)), execlet_run_task);
    }
};

// Allows Folly semifutures (which can be retrieved from Folly tasks via the `semi()` method) to be
// awaited.
template <typename Result, typename Future>
class AwaitTransformer<folly::SemiFuture<Result>, Future> {
    AwaitTransformer() = delete;

   public:
    static auto await_transform(RustPromise<Future>& promise,
                                folly::SemiFuture<Result>&& semifuture) noexcept {
        return std::move(semifuture).via(new FollyExeclet(promise.execlet()));
    }
};

}  // namespace async
}  // namespace rust

#endif  // RUST_CXX_ASYNC_FOLLY_H
