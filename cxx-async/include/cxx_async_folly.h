// cxx-async2/include/cxx_async_folly.h

#ifndef CXX_ASYNC2_CXX_ASYNC_FOLLY_H
#define CXX_ASYNC2_CXX_ASYNC_FOLLY_H

#include <folly/Executor.h>
#include <folly/Try.h>
#include <folly/executors/ManualExecutor.h>
#include <folly/experimental/coro/Task.h>
#include <folly/experimental/coro/ViaIfAsync.h>
#include <mutex>
#include <queue>
#include "cxx_async.h"
#include "folly/src/main.rs.h"

namespace cxx {
namespace async {

class Execlet : public folly::Executor {
    rust::Box<RustExecletF64> m_rust_execlet;

   public:
    Execlet(rust::Box<RustExecletF64>&& rust_execlet) : m_rust_execlet(std::move(rust_execlet)) {}

    virtual void add(folly::Func task) {
        m_rust_execlet->submit(reinterpret_cast<uint8_t*>(new folly::Func(std::move(task))));
    }

    void send(double&& result) const {
        RustFutureResult<RustFutureF64> rust_result;
        new (&rust_result.m_result) double(std::move(result));
        m_rust_execlet->send(&rust_result.m_result);
    }
};

inline rust::Box<RustFutureF64> folly_task_to_rust_future(folly::coro::Task<double>&& task) {
    RustExecletBundleF64 bundle = static_cast<RustFutureF64*>(nullptr)->execlet();
    folly::Executor::KeepAlive<Execlet> execlet(new Execlet(std::move(bundle.execlet)));
    folly::coro::TaskWithExecutor<double> boundTask = std::move(task).scheduleOn(execlet);
    std::move(boundTask).start([execlet = std::move(execlet)](folly::Try<double>&& result) {
        // TODO(pcwalton): Exceptions.
        execlet->send(std::move(result.value()));
    });
    return std::move(bundle.future);
}

inline void execlet_run_task(const uint8_t* execlet, uint8_t* task_ptr) {
    folly::Function<void()>* task = reinterpret_cast<folly::Function<void()>*>(task_ptr);
    (*task)();
    delete task;
}

template <typename Result>
class AwaitTransformer<folly::coro::Task<Result>> {
    AwaitTransformer() = delete;

   public:
    static rust::Box<RustFutureF64> await_transform(folly::coro::Task<Result>&& task) noexcept {
        return folly_task_to_rust_future(std::move(task));
    }
};

}  // namespace async
}  // namespace cxx

#endif
