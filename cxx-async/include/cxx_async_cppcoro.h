// cxx-async/include/cxx_async_cppcoro.h

#ifndef CXX_ASYNC_CXX_ASYNC_CPPCORO_H
#define CXX_ASYNC_CXX_ASYNC_CPPCORO_H

#include <cppcoro/awaitable_traits.hpp>

namespace cxx {
namespace async {

template <typename Future>
struct RustExecletBundle {};

template <typename Future>
class Execlet {};

}  // namespace async
}  // namespace cxx

template <>
struct cppcoro::awaitable_traits<rust::Box<RustFutureF64>&&> {
    typedef double await_result_t;
};

extern "C" inline void cxxasync_execlet_run_task(const uint8_t* execlet, uint8_t* task_ptr) {}

#endif  // CXX_ASYNC_CXX_ASYNC_CPPCORO_H
