// cxx-async2/include/cxx_async_cppcoro.h

#ifndef CXX_ASYNC2_CXX_ASYNC_CPPCORO_H
#define CXX_ASYNC2_CXX_ASYNC_CPPCORO_H

#include <cppcoro/awaitable_traits.hpp>

template <>
struct cppcoro::awaitable_traits<rust::Box<RustFutureF64>&&> {
    typedef double await_result_t;
};

#endif
