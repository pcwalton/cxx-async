// cxx-async/include/rust/cxx_async.h

#ifndef RUST_CXX_ASYNC_H
#define RUST_CXX_ASYNC_H

#include <atomic>
#include <cstdint>
#include <cstring>
#include <exception>
#include <experimental/coroutine>
#include <functional>
#include <memory>
#include <mutex>
#include <new>
#include <stdexcept>
#include <type_traits>
#include <utility>
#include "rust/cxx.h"

#define CXXASYNC_ASSERT(cond) ::rust::async::cxxasync_assert(cond)

#define CXXASYNC_DEFINE_FUTURE(name, type)       \
    template <>                                  \
    struct rust::async::RustFutureTraits<name> { \
        typedef type Result;                     \
    };

namespace rust {
namespace async {

// Must match the definition in `cxx_async/src/lib.rs`.
template <typename Future>
class RustFutureTraits {};

template <typename Future>
struct Vtable;

template <typename Future>
class FutureVtableProvider {
   public:
    static const rust::async::Vtable<Future>* vtable();
};

// FIXME(pcwalton): Is making these incomplete types the right thing to do? It requires the macro to
// define drop glue for the `rust::Box` destructor to call, and that's a bit messy.
template <typename Future>
class RustSender;
template <typename Future>
struct RustOneshot;
template <typename Future>
class RustPromise;

template <typename Future>
using RustResultFor = typename RustFutureTraits<Future>::Result;

class SuspendedCoroutine;

// This has to be separate from `rust::Error` because constructing a `rust::Error` is private API.
class Error final : public std::exception {
    char* m_message;
    void copy_from(const char* message) {
        size_t len = std::strlen(message) + 1;
        m_message = new char[len];
        std::memcpy(reinterpret_cast<void*>(m_message), message, len);
    }
    void destroy() {
        if (m_message != nullptr)
            delete[] m_message;
    }
    Error(const char* message) { copy_from(message); }
    template <typename Future>
    friend class RustFutureReceiver;

   public:
    Error(const Error& other) { copy_from(other.m_message); }
    Error(Error&& other) : m_message(other.m_message) { other.m_message = nullptr; }
    ~Error() noexcept override { destroy(); }
    Error& operator=(const Error& other) {
        destroy();
        copy_from(other.m_message);
        return *this;
    }
    Error& operator=(Error&& other) noexcept {
        m_message = other.m_message;
        other.m_message = nullptr;
        return *this;
    }
    const char* what() const noexcept override { return m_message; }
};

// Exception customization point. This works just like `rust::behavior::trycatch` [1], except that
// it allows the `trycatch` behavior to declared anywhere before the future is used, reducing header
// file ordering issues.
//
// Define a specialization of the `TryCatch` with this signature in order to customize exception
// handling:
//
//      namespace behavior {
//      template<typename T>
//      struct TryCatch<T, Custom> {
//          template<typename Try, typename Fail>
//          static void trycatch(Try&& func, Fail&& fail) noexcept {
//              ...
//          }
//      };
//      } // end namespace behavior
//
// The useless-seeming `T` type parameter is required on the outer struct so that the name lookup
// inside the `Future` template will happen in the second phase (instantiation time) as opposed to
// the first phase (declaration time).
//
// This has to be separate from `rust::behavior::trycatch` because `cxx` won't always generate the
// default definition of that function, and we can't force it to.
//
// [1]: https://cxx.rs/binding/result.html
namespace behavior {

struct Custom {};

template <typename T, typename C>
struct TryCatch {
    template <typename Try, typename Fail>
    static void trycatch(Try&& func, Fail&& fail) noexcept {
        try {
            func();
        } catch (const std::exception& e) {
            fail(e.what());
        }
    }
};

}  // namespace behavior

void cxxasync_assert(bool cond);

// Execlet API
struct RustExeclet;
extern "C" {
// Creates a new execlet.
RustExeclet* cxxasync_execlet_create();
// Decrements the reference count on an execlet and frees it if the count hits zero.
void cxxasync_execlet_release(RustExeclet* self);
// Submit a task to the execlet. This internally bumps the reference count.
void cxxasync_execlet_submit(RustExeclet* self, void (*run)(void*), void* task);
}

// Execlet
class Execlet {
    RustExeclet* m_priv;

    Execlet(const Execlet&) = delete;
    Execlet& operator=(const Execlet&) = delete;

   public:
    Execlet() : m_priv(cxxasync_execlet_create()) {}

    ~Execlet() { cxxasync_execlet_release(m_priv); }

    void submit(void* task, void (*run)(void*)) noexcept {
        cxxasync_execlet_submit(m_priv, run, task);
    }

    RustExeclet* raw() { return m_priv; }
};

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

// Downstream libraries can customize this by adding template specializations.
template <typename Awaiter, typename Future>
class AwaitTransformer {
    AwaitTransformer() = delete;

   public:
    static auto await_transform(RustPromise<Future>& promise, Awaiter&& awaitable) noexcept {
        return std::move(awaitable);
    }
};

template <typename Future>
struct Vtable {
    RustOneshot<Future> (*channel)(RustExeclet* execlet);
    void (*sender_send)(RustSender<Future>& self, uint32_t status, const void* value);
    uint32_t (*future_poll)(Future& self, void* result, const void* waker_data);
};

template <typename Future>
struct RustOneshot {
    rust::Box<Future> future;
    rust::Box<RustSender<Future>> sender;
};

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
                throw Error(m_result.m_exception.c_str());
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

// This is like `std::experimental::coroutine_handle<void>`, but it doesn't *have* to be a
// coroutine handle.
class Continuation {
    Continuation(const Continuation&) = delete;
    void operator=(const Continuation&) = delete;

   protected:
    Continuation() {}

   public:
    virtual ~Continuation() {}
    virtual void resume() = 0;
    virtual void destroy() = 0;
};

class CoroutineHandleContinuation : public Continuation {
    std::experimental::coroutine_handle<void> m_next;

   public:
    CoroutineHandleContinuation(std::experimental::coroutine_handle<void>&& next) : m_next(next) {}
    virtual void resume() { m_next.resume(); }
    virtual void destroy() { m_next.destroy(); }
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
    std::unique_ptr<Continuation> m_next;
    WakeFn m_wake_fn;

    void forget_coroutine_handle() { m_next.reset(); }

   public:
    SuspendedCoroutine(std::unique_ptr<Continuation>&& next, WakeFn&& wake_fn)
        : m_refcount(1), m_next(std::move(next)), m_wake_fn(std::move(wake_fn)) {}

    ~SuspendedCoroutine() {
        if (m_next) {
            m_next->destroy();
            m_next.reset();
        }
    }

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

    // Performs the initial poll needed when we go to sleep for the first time. Returns true if we
    // should go to sleep and false otherwise.
    //
    // Consumes the `this` reference.
    bool initial_suspend() {
        FutureWakeStatus status = wake();

        // Tricky: if the future is already complete, we won't go to sleep, which means we won't
        // resume, so unless we intervene like this nothing will stop our destructor from
        // destroying the coroutine handle.
        bool done = wake_status_is_done(status);
        if (done)
            forget_coroutine_handle();
        release();
        return !done;
    }

    void resume() {
        CXXASYNC_ASSERT(bool(m_next));
        std::unique_ptr<Continuation> next = std::move(m_next);
        forget_coroutine_handle();
        next->resume();
    }
};

// Promise object that manages the oneshot channel that is returned to Rust when Rust calls a C++
// coroutine.
template <typename Future>
class RustPromise {
    typedef RustOneshot<Future> Oneshot;
    typedef RustResultFor<Future> Result;

    // Don't change the order of these!
    Execlet m_execlet;
    Oneshot m_oneshot;

    RustPromise(const RustPromise&) = delete;
    RustPromise& operator=(const RustPromise&) = delete;

   public:
    RustPromise()
        : m_execlet(),
          m_oneshot(FutureVtableProvider<Future>::vtable()->channel(m_execlet.raw())) {}

    rust::Box<Future> get_return_object() noexcept { return std::move(m_oneshot.future); }

    std::experimental::suspend_never initial_suspend() const noexcept { return {}; }
    std::experimental::suspend_never final_suspend() const noexcept { return {}; }
    std::experimental::coroutine_handle<> unhandled_done() noexcept { return {}; }

    void return_value(Result&& value) {
        RustFutureResult<Future> result;
        new (&result.m_result) Result(std::move(value));
        FutureVtableProvider<Future>::vtable()->sender_send(
            *m_oneshot.sender, static_cast<uint32_t>(FuturePollStatus::Complete),
            reinterpret_cast<const uint8_t*>(&result));
    }

    void unhandled_exception() noexcept {
        behavior::TryCatch<Future, behavior::Custom>::trycatch(
            []() { std::rethrow_exception(std::current_exception()); },
            [&](const char* what) {
                const Vtable<Future>* vtable = FutureVtableProvider<Future>::vtable();
                vtable->sender_send(*m_oneshot.sender,
                                    static_cast<uint32_t>(FuturePollStatus::Error), what);
            });
    }

    Execlet& execlet() noexcept { return m_execlet; }

    // Customization point for library integration (e.g. folly).
    template <typename Awaiter>
    auto await_transform(Awaiter&& awaitable) noexcept {
        return AwaitTransformer<Awaiter, Future>::await_transform(*this, std::move(awaitable));
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

    m_status = static_cast<FuturePollStatus>(
        FutureVtableProvider<Future>::vtable()->future_poll(*m_future, &m_result, coroutine));
    return static_cast<FutureWakeStatus>(m_status);
}

template <typename Future>
inline bool RustAwaiter<Future>::await_suspend(std::experimental::coroutine_handle<void> next) {
    std::weak_ptr<RustFutureReceiver<Future>> weak_receiver = m_receiver;
    SuspendedCoroutine* coroutine = new SuspendedCoroutine(
        std::make_unique<CoroutineHandleContinuation>(std::move(next)),
        [weak_receiver = std::move(weak_receiver)](SuspendedCoroutine* coroutine) {
            std::shared_ptr<RustFutureReceiver<Future>> receiver = weak_receiver.lock();
            // This rarely ever happens in practice, but I think it can.
            if (!receiver)
                return FutureWakeStatus::Dead;
            return receiver->wake(coroutine->add_ref());
        });
    return coroutine->initial_suspend();
}

}  // namespace async

// This can't be in the `rust::async` namespace because it relies on ADL on `rust::Box<Future>` to
// be found.
template <typename Future>
inline async::RustAwaiter<Future> operator co_await(Box<Future>&& future) noexcept {
    return async::RustAwaiter(std::move(future));
}

}  // namespace rust

template <typename Future, typename... Args>
struct std::experimental::coroutine_traits<rust::Box<Future>, Args...> {
    using promise_type = rust::async::RustPromise<Future>;
};

#endif  // RUST_CXX_ASYNC_H
