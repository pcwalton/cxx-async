// cxx-async/examples/cppcoro/src/cppcoro_example.cpp
//
// An example showing how to interoperate with `cppcoro`.

#include "cppcoro_example.h"
#include <cppcoro/fmap.hpp>
#include <cppcoro/schedule_on.hpp>
#include <cppcoro/static_thread_pool.hpp>
#include <cppcoro/sync_wait.hpp>
#include <cppcoro/task.hpp>
#include <cppcoro/when_all.hpp>
#include <cstdlib>
#include <exception>
#include <experimental/coroutine>
#include <functional>
#include <iosfwd>
#include <new>
#include <stdexcept>
#include <string>
#include <tuple>
#include <type_traits>
#include <vector>
#include "cxx-async-example-cppcoro/src/main.rs.h"
#include "cxx_async.h"
#include "example.h"
#include "rust/cxx.h"

CXXASYNC_DEFINE_FUTURE(RustFutureF64, double);
CXXASYNC_DEFINE_FUTURE(RustFutureString, rust::String);

const size_t EXAMPLE_SPLIT_LIMIT = 32;
const size_t EXAMPLE_ARRAY_SIZE = 16384;

// TODO(pcwalton): It'd be nice to be able to spawn the extra threads that `dot_product` creates on
// a Rust thread pool instead. There has to be some kind of thread pool for this example to actually
// test parallelism, though.
cppcoro::static_thread_pool g_thread_pool;

// Multithreaded dot product computation.
static cppcoro::task<double> dot_product_inner(const double a[], const double b[], size_t count) {
    if (count > EXAMPLE_SPLIT_LIMIT) {
        size_t half_count = count / 2;
        auto [first, second] = co_await cppcoro::when_all(
            cppcoro::schedule_on(g_thread_pool, dot_product_inner(a, b, half_count)),
            dot_product_inner(a + half_count, b + half_count, count - half_count));
        co_return first + second;
    }

    double sum = 0.0;
    for (size_t i = 0; i < count; i++)
        sum += a[i] * b[i];
    co_return sum;
}

static cppcoro::task<double> dot_product() {
    Xorshift rand;
    std::vector<double> array_a, array_b;
    for (size_t i = 0; i < EXAMPLE_ARRAY_SIZE; i++) {
        array_a.push_back((double)rand.next());
        array_b.push_back((double)rand.next());
    }

    co_return co_await dot_product_inner(&array_a[0], &array_b[0], array_a.size());
}

rust::Box<RustFutureF64> cppcoro_dot_product() {
    co_return co_await dot_product();
}

double cppcoro_call_rust_dot_product() {
    return cppcoro::sync_wait(rust_dot_product());
}

double cppcoro_schedule_rust_dot_product() {
    return cppcoro::sync_wait(cppcoro::schedule_on(g_thread_pool, rust_dot_product()));
}

rust::Box<RustFutureF64> cppcoro_not_product() {
    if (true)
        throw std::runtime_error("kaboom");
    co_return 1.0;  // Just to make this function a coroutine.
}

rust::String cppcoro_call_rust_not_product() {
    try {
        rust::Box<RustFutureF64> oneshot_receiver = rust_not_product();
        cppcoro::sync_wait(std::move(oneshot_receiver));
        std::terminate();
    } catch (const std::exception& error) {
        return rust::String(error.what());
    }
}

rust::Box<RustFutureString> cppcoro_ping_pong(int i) {
    std::string string(co_await rust_cppcoro_ping_pong(i));
    co_return std::move(string) + "pong ";
}
