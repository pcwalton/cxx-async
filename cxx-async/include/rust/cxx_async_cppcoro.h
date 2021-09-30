// cxx-async/include/rust/cxx_async_cppcoro.h

#ifndef RUST_CXX_ASYNC_CPPCORO_H
#define RUST_CXX_ASYNC_CPPCORO_H

#include <cppcoro/awaitable_traits.hpp>

namespace rust {
namespace async {

template <typename Future>
struct RustExecletBundle {};

template <typename Future>
class Execlet {};

}  // namespace async
}  // namespace rust

extern "C" inline void cxxasync_execlet_run_task(const uint8_t* execlet, uint8_t* task_ptr) {}

#endif  // RUST_CXX_ASYNC_CPPCORO_H
