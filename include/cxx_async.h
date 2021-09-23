// cxx-async2/include/cxx_async.h

#ifndef CXX_ASYNC2_CXX_ASYNC_H
#define CXX_ASYNC2_CXX_ASYNC_H

#include <atomic>
#include <cstdint>
#include <cstdlib>
#include <experimental/coroutine>
#include <functional>
#include <iostream>
#include <optional>
#include "cxx-async2/src/main.rs.h"
#include "cxx_async_waker.h"
#include "rust/cxx.h"

// TODO(pcwalton): Namespace.

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
using RustOneshotResultFor =
    typename RustFutureResultTypeExtractor<decltype(&Future::channel)>::Result;

class SuspendedCoroutine;

void cxxasync_assert(bool cond);

enum class FutureStatus {
    Pending,
    Complete,
    Error,
};

template <typename Future>
union RustFutureResult {
    RustOneshotResultFor<Future> m_result;
    rust::String m_exception;

    RustFutureResult() {}
    // When you use this type, you must manually drop.
    ~RustFutureResult() {}
};

template <typename Future>
class RustAwaiter {
    typedef RustOneshotResultFor<Future> Result;

    /*
    friend uint8_t* cxxasync_coroutine_handle_clone(uint8_t* address);
    friend void cxxasync_coroutine_handle_wake(uint8_t* address);
    friend void cxxasync_coroutine_handle_wake_by_ref(uint8_t* address);
    friend void cxxasync_coroutine_handle_drop(uint8_t* address);
    */
    friend class SuspendedCoroutine;

    rust::Box<Future> m_future;
    RustFutureResult<Future> m_result;
    FutureStatus m_status;

    RustAwaiter(const RustAwaiter&) = delete;
    void operator=(const RustAwaiter&) = delete;

   public:
    RustAwaiter(rust::Box<Future>&& future) : m_future(std::move(future)) {}

    bool await_ready() noexcept {
        // We could poll here, but let's not. Assume that polling is more expensive than creating
        // the coroutine state.
        return false;
    }

    bool await_suspend(std::experimental::coroutine_handle<void> next);

    Result&& await_resume() {
        switch (m_status) {
            case FutureStatus::Complete:
                return std::move(m_result.m_result);
            case FutureStatus::Error:
                throw std::runtime_error(std::string(m_result.m_exception));
            case FutureStatus::Pending:
                CXXASYNC_ASSERT(false);
                std::terminate();
        }
    }
};

// Wrapper object that encapsulates a suspended coroutine. This is the waker that is exposed to
// Rust.
class SuspendedCoroutine {
    SuspendedCoroutine(const SuspendedCoroutine&) = delete;
    void operator=(const SuspendedCoroutine&) = delete;

   public:
    typedef std::function<FutureStatus(SuspendedCoroutine*)> PollFn;

    std::atomic<uintptr_t> m_refcount;
    std::optional<std::experimental::coroutine_handle<void>> m_next;
    PollFn m_poll_fn;

    SuspendedCoroutine(std::experimental::coroutine_handle<void>&& next, PollFn&& poll_fn)
        : m_refcount(1), m_next(next), m_poll_fn(std::move(poll_fn)) {}

    ~SuspendedCoroutine() {
        if (m_next) {
            m_next->destroy();
            m_next.reset();
        }
    }

    void preserve_coroutine_handle() { m_next.reset(); }

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
    FutureStatus poll() { return m_poll_fn(this); }

    void resume() {
        CXXASYNC_ASSERT(m_next.has_value());
        std::experimental::coroutine_handle<void>&& next = std::move(*m_next);
        preserve_coroutine_handle();
        next.resume();
    }
};

template <typename Future>
class RustPromise {
    typedef RustOneshotFor<Future> Oneshot;
    typedef RustOneshotResultFor<Future> Result;

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
        m_oneshot.sender->send(static_cast<uint32_t>(FutureStatus::Complete),
                               reinterpret_cast<const uint8_t*>(&result));
    }

    void unhandled_exception() noexcept {
        try {
            std::rethrow_exception(std::current_exception());
        } catch (const std::exception& exception) {
            m_oneshot.sender->send(static_cast<uint32_t>(FutureStatus::Error),
                                   reinterpret_cast<const uint8_t*>(exception.what()));
        } catch (...) {
            m_oneshot.sender->send(static_cast<uint32_t>(FutureStatus::Error),
                                   reinterpret_cast<const uint8_t*>("Unhandled C++ exception"));
        }
    }

    // Some libraries, like libunifex, need this.
    template <typename Awaitable>
    Awaitable await_transform(Awaitable&& awaitable) noexcept {
        return std::move(awaitable);
    }
};

template <typename Future>
inline bool RustAwaiter<Future>::await_suspend(std::experimental::coroutine_handle<void> next) {
    SuspendedCoroutine* coroutine =
        new SuspendedCoroutine(std::move(next), [&](SuspendedCoroutine* coroutine) {
            return m_status = static_cast<FutureStatus>(
                       m_future->poll(reinterpret_cast<uint8_t*>(&m_result),
                                      reinterpret_cast<uint8_t*>(coroutine->add_ref())));
        });
    FutureStatus status = coroutine->poll();

    // Tricky: if the future is already complete, we won't go to sleep, which means we won't
    // resume, so unless we intervene like this nothing will stop the destructor of
    // `SuspendedCoroutine` from destroying the coroutine handle.
    if (status != FutureStatus::Pending)
        coroutine->preserve_coroutine_handle();
    coroutine->release();

    return status == FutureStatus::Pending;
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
