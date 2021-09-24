// cxx-async2/include/cxx_async.h

#ifndef CXX_ASYNC2_CXX_ASYNC_H
#define CXX_ASYNC2_CXX_ASYNC_H

#include <atomic>
#include <cstdint>
#include <cstdlib>
#include <experimental/coroutine>
#include <functional>
#include <iostream>
#include <memory>
#include <mutex>
#include <optional>
#include "cxx_async_waker.h"
#include "rust/cxx.h"

#define CXXASYNC_ASSERT(cond) ::cxx::async::cxxasync_assert(cond)

namespace cxx {
namespace async {

// Given a future type, fetches the oneshot channel type that matches its output.
template <typename Future>
using RustOneshotFor = decltype(static_cast<Future*>(nullptr)->channel(nullptr));

// Given a future type, fetches the result type that matches its output.
//
// This extracts the type of the `value` parameter from the `channel` method using
// the technique described here: https://stackoverflow.com/a/28033314
template <typename Fn>
struct RustFutureResultTypeExtractor;
template <typename Future, typename TheResult, typename TheOneshot>
struct RustFutureResultTypeExtractor<TheOneshot (Future::*)(const TheResult*) const noexcept> {
    typedef TheResult Result;
};
template <typename Future>
using RustResultFor =
    typename RustFutureResultTypeExtractor<decltype(&Future::channel)>::Result;

class SuspendedCoroutine;

void cxxasync_assert(bool cond);

enum class FuturePollStatus {
    Pending,
    Complete,
    Error,
};

enum class FutureWakeStatus {
    Pending,
    Complete,
    Error,
    Dead,
};

inline bool wake_status_is_done(FutureWakeStatus status) {
    return status == FutureWakeStatus::Complete || status == FutureWakeStatus::Error;
}

// A temporary place to hold future results or errors that are sent to or returned from Rust.
template <typename Future>
union RustFutureResult {
    RustResultFor<Future> m_result;
    rust::String m_exception;

    // When using this type, you must fill `m_result` or `m_exception` manually via placement new.
    RustFutureResult() {}
    // When using this type, you must manually drop the contents.
    ~RustFutureResult() {}
};

template <typename Future>
class RustFutureReceiver {
    typedef RustResultFor<Future> Result;

    std::mutex m_lock;
    rust::Box<Future> m_future;
    RustFutureResult<Future> m_result;
    FuturePollStatus m_status;

    RustFutureReceiver(const RustFutureReceiver&) = delete;
    void operator=(const RustFutureReceiver&) = delete;

   public:
    RustFutureReceiver(rust::Box<Future>&& future)
        : m_lock(), m_future(std::move(future)), m_status(FuturePollStatus::Pending) {}

    // Consumes the `coroutine` reference (so you probably want to addref it first).
    FutureWakeStatus wake(SuspendedCoroutine* coroutine);

    Result&& get_result() {
        // Safe to use without taking the lock because the caller asserts that the future has
        // already completed.
        switch (m_status) {
            case FuturePollStatus::Complete:
                return std::move(m_result.m_result);
            case FuturePollStatus::Error:
                throw std::runtime_error(std::string(m_result.m_exception));
            case FuturePollStatus::Pending:
                CXXASYNC_ASSERT(false);
                std::terminate();
        }
    }
};

template <typename Future>
class RustAwaiter {
    typedef RustResultFor<Future> Result;

    friend class SuspendedCoroutine;

    std::shared_ptr<RustFutureReceiver<Future>> m_receiver;

    RustAwaiter(const RustAwaiter&) = delete;
    void operator=(const RustAwaiter&) = delete;

   public:
    RustAwaiter(rust::Box<Future>&& future)
        : m_receiver(std::make_shared<RustFutureReceiver<Future>>(std::move(future))) {}

    bool await_ready() noexcept {
        // We could poll here, but let's not. Assume that polling is more expensive than creating
        // the coroutine state.
        return false;
    }

    bool await_suspend(std::experimental::coroutine_handle<void> next);

    Result&& await_resume() { return m_receiver->get_result(); }
};

// Wrapper object that encapsulates a suspended coroutine. This is the waker that is exposed to
// Rust.
//
// This object is *manually* reference counted via `add_ref()` and `release()`, to match the
// `RawWaker` interface that Rust expects.
class SuspendedCoroutine {
    SuspendedCoroutine(const SuspendedCoroutine&) = delete;
    void operator=(const SuspendedCoroutine&) = delete;

    typedef std::function<FutureWakeStatus(SuspendedCoroutine*)> WakeFn;

    std::atomic<uintptr_t> m_refcount;
    std::optional<std::experimental::coroutine_handle<void>> m_next;
    WakeFn m_wake_fn;

   public:
    SuspendedCoroutine(std::experimental::coroutine_handle<void>&& next, WakeFn&& wake_fn)
        : m_refcount(1), m_next(next), m_wake_fn(std::move(wake_fn)) {}

    ~SuspendedCoroutine() {
        if (m_next) {
            m_next->destroy();
            m_next.reset();
        }
    }

    void forget_coroutine_handle() { m_next.reset(); }

    SuspendedCoroutine* add_ref() {
        m_refcount.fetch_add(1);
        return this;
    }

    void release() {
        uintptr_t last_refcount = m_refcount.fetch_sub(1);
        CXXASYNC_ASSERT(last_refcount > 0);
        if (last_refcount == 1)
            delete this;
    }

    // Does not consume the `this` reference.
    FutureWakeStatus wake() { return m_wake_fn(this); }

    void resume() {
        CXXASYNC_ASSERT(m_next.has_value());
        std::experimental::coroutine_handle<void>&& next = std::move(*m_next);
        forget_coroutine_handle();
        next.resume();
    }
};

// Promise object that manages the oneshot channel that is returned to Rust when Rust calls a C++
// coroutine.
template <typename Future>
class RustPromise {
    typedef RustOneshotFor<Future> Oneshot;
    typedef RustResultFor<Future> Result;

    Oneshot m_oneshot;

   public:
    RustPromise() : m_oneshot(static_cast<Future*>(nullptr)->channel(nullptr)) {}

    rust::Box<Future> get_return_object() noexcept { return std::move(m_oneshot.future); }

    std::experimental::suspend_never initial_suspend() const noexcept { return {}; }
    std::experimental::suspend_never final_suspend() const noexcept { return {}; }
    std::experimental::coroutine_handle<> unhandled_done() noexcept { return {}; }

    void return_value(Result&& value) {
        RustFutureResult<Future> result;
        new (&result.m_result) Result(std::move(value));
        m_oneshot.sender->send(static_cast<uint32_t>(FuturePollStatus::Complete),
                               reinterpret_cast<const uint8_t*>(&result));
    }

    void unhandled_exception() noexcept {
        try {
            std::rethrow_exception(std::current_exception());
        } catch (const std::exception& exception) {
            m_oneshot.sender->send(static_cast<uint32_t>(FuturePollStatus::Error),
                                   reinterpret_cast<const uint8_t*>(exception.what()));
        } catch (...) {
            m_oneshot.sender->send(static_cast<uint32_t>(FuturePollStatus::Error),
                                   reinterpret_cast<const uint8_t*>("Unhandled C++ exception"));
        }
    }

    // Some libraries, like libunifex, need this.
    template <typename Awaitable>
    Awaitable await_transform(Awaitable&& awaitable) noexcept {
        return std::move(awaitable);
    }
};

// Consumes the `coroutine` reference (so you probably want to addref it first).
template <typename Future>
FutureWakeStatus RustFutureReceiver<Future>::wake(SuspendedCoroutine* coroutine) {
    std::lock_guard<std::mutex> guard(m_lock);

    // Have we already polled this future to completion? If so, don't poll again.
    if (m_status != FuturePollStatus::Pending) {
        coroutine->release();
        return FutureWakeStatus::Dead;
    }

    m_status = static_cast<FuturePollStatus>(m_future->poll(reinterpret_cast<uint8_t*>(&m_result),
                                                            reinterpret_cast<uint8_t*>(coroutine)));
    return static_cast<FutureWakeStatus>(m_status);
}

template <typename Future>
inline bool RustAwaiter<Future>::await_suspend(std::experimental::coroutine_handle<void> next) {
    std::weak_ptr<RustFutureReceiver<Future>> weak_receiver = m_receiver;
    SuspendedCoroutine* coroutine = new SuspendedCoroutine(
        std::move(next), [weak_receiver = std::move(weak_receiver)](SuspendedCoroutine* coroutine) {
            std::shared_ptr<RustFutureReceiver<Future>> receiver = weak_receiver.lock();
            // This rarely ever happens in practice, but I think it can.
            if (!receiver)
                return FutureWakeStatus::Dead;
            return receiver->wake(coroutine->add_ref());
        });

    FutureWakeStatus status = coroutine->wake();

    // Tricky: if the future is already complete, we won't go to sleep, which means we won't
    // resume, so unless we intervene like this nothing will stop the destructor of
    // `SuspendedCoroutine` from destroying the coroutine handle.
    bool done = wake_status_is_done(status);
    if (done)
        coroutine->forget_coroutine_handle();
    coroutine->release();
    return !done;
}

}  // namespace async
}  // namespace cxx

// FIXME(pcwalton): Why does this have to be outside the namespace?
template <typename Future>
inline cxx::async::RustAwaiter<Future> operator co_await(rust::Box<Future>&& future) noexcept {
    return cxx::async::RustAwaiter(std::move(future));
}

template <typename Future, typename... Args>
struct std::experimental::coroutine_traits<rust::Box<Future>, Args...> {
    using promise_type = cxx::async::RustPromise<Future>;
};

#endif
