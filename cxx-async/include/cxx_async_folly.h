// cxx-async2/include/cxx_async_folly.h

#ifndef CXX_ASYNC2_CXX_ASYNC_FOLLY_H
#define CXX_ASYNC2_CXX_ASYNC_FOLLY_H

#include <folly/Executor.h>
#include <folly/Try.h>
#include <folly/executors/ManualExecutor.h>
#include <folly/experimental/coro/Task.h>
#include <folly/experimental/coro/ViaIfAsync.h>
#include <iostream>
#include <mutex>
#include <queue>
#include "cxx_async.h"
#include "folly/src/main.rs.h"

namespace cxx {
namespace async {

extern "C" inline void execlet_run_task(void* task_ptr);

template <typename Future, typename Execlet>
struct RustExecletBundle {
    rust::Box<Future> future;
    rust::Box<Execlet> execlet;
};

template <typename Future>
class Execlet : public folly::Executor {
    typedef RustExecletFor<Future> RustExeclet;
    typedef RustResultFor<Future> Result;

    rust::Box<RustExeclet> m_rust_execlet;

   public:
    Execlet(rust::Box<RustExeclet>&& rust_execlet) : m_rust_execlet(std::move(rust_execlet)) {}

    virtual void add(folly::Func task) {
        RustFutureTraits<Future>::vtable()->execlet_submit(*m_rust_execlet, execlet_run_task,
                                                           new folly::Func(std::move(task)));
    }

    void send_value(Result&& result) const {
        RustFutureResult<Future> rust_result;
        new (&rust_result.m_result) Result(std::move(result));
        RustFutureTraits<Future>::vtable()->execlet_send(
            *m_rust_execlet, static_cast<uint32_t>(FuturePollStatus::Complete),
            &rust_result.m_result);
    }

    void send_exception(const char *what) const {
        RustFutureTraits<Future>::vtable()->execlet_send(
            *m_rust_execlet, static_cast<uint32_t>(FuturePollStatus::Error), what);
    }
};

// Converts a Folly Task to a Rust Future by wrapping the former in an execlet.
//
// Usually you don't need to call this manually, because you can just `co_await` a Folly Task
// inside a coroutine that returns a Rust Future (thanks to the magic of await transformers). For
// example, instead of writing `cxx::async::folly_task_to_rust_future(foo())` you can just write
// `co_return co_await foo();`.
template <typename Future>
rust::Box<Future> folly_task_to_rust_future(folly::coro::Task<RustResultFor<Future>>&& task) {
    typedef RustExecletFor<Future> RustExeclet;
    typedef RustResultFor<Future> Result;

    auto bundle = RustFutureTraits<Future>::vtable()->execlet();
    folly::Executor::KeepAlive<Execlet<Future>> execlet(
        new Execlet<Future>(std::move(bundle.execlet)));
    folly::coro::TaskWithExecutor<Result> boundTask = std::move(task).scheduleOn(execlet);
    std::move(boundTask).start([execlet = std::move(execlet)](folly::Try<Result>&& result) {
        if (result.hasException())
            execlet->send_exception(result.exception().get_exception()->what());
        else
            execlet->send_value(std::move(result.value()));
    });

    return std::move(bundle.future);
}

extern "C" inline void execlet_run_task(void* task_ptr) {
    folly::Function<void()>* task = reinterpret_cast<folly::Function<void()>*>(task_ptr);
    (*task)();
    delete task;
}

template <typename Result, typename Future>
class AwaitTransformer<folly::coro::Task<Result>, Future> {
    AwaitTransformer() = delete;

   public:
    static auto await_transform(folly::coro::Task<Result>&& task) noexcept {
        return folly_task_to_rust_future<Future>(std::move(task));
    }
};

}  // namespace async
}  // namespace cxx

#endif
