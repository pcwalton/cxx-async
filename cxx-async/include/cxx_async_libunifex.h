// cxx-async2/include/cxx_async_libunifex.h

#ifndef CXX_ASYNC2_CXX_ASYNC_LIBUNIFEX_H
#define CXX_ASYNC2_CXX_ASYNC_LIBUNIFEX_H

#include <experimental/coroutine>
#include <unifex/sender_concepts.hpp>
#include "cxx_async.h"
#include "rust/cxx.h"

namespace cxx {
namespace async {

template <typename Future, typename Receiver>
class RustOperation {
    rust::Box<Future> m_future;
    Receiver m_receiver;

    class NoopTask {
       public:
        using promise_type = std::experimental::noop_coroutine_promise;
    };

    NoopTask make_task() noexcept {
        try {
            RustResultFor<Future> result = co_await std::move(m_future);
            unifex::set_value(std::move(m_receiver), std::move(result));
        } catch (...) {
            unifex::set_error(std::move(m_receiver), std::current_exception());
        }
        co_return;
    }

    RustOperation(const RustOperation&) = delete;
    void operator=(const RustOperation&) = delete;

   public:
    RustOperation(rust::Box<Future>&& future, Receiver&& receiver)
        : m_future(std::move(future)), m_receiver(std::move(receiver)) {}

    void start() noexcept { make_task(); }
};

}  // namespace async
}  // namespace cxx

template <typename Future, typename Receiver>
cxx::async::RustOperation<Future, Receiver> tag_invoke(unifex::tag_t<unifex::connect>,
                                                       rust::Box<Future>&& future,
                                                       Receiver&& receiver) {
    return cxx::async::RustOperation<Future, Receiver>(std::move(future), std::move(receiver));
}

// Rust Futures are unifex senders.
template <typename Future>
struct unifex::sender_traits<rust::Box<Future>> {
    template <template <typename...> class Variant, template <typename...> class Tuple>
    using value_types = Variant<Tuple<cxx::async::RustResultFor<Future>>>;
    template <template <typename...> class Variant>
    using error_types = Variant<>;
    static constexpr bool sends_done = true;
};

#endif
