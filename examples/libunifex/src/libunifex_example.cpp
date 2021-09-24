// cxx-async2/examples/libunifex/src/libunifex_example.cpp

#include <cstdlib>
#include <iostream>
#include <unifex/config.hpp>
#include <unifex/coroutine.hpp>
#include <unifex/execute.hpp>
#include <unifex/inline_scheduler.hpp>
#include <unifex/scheduler_concepts.hpp>
#include <unifex/sender_concepts.hpp>
#include <unifex/static_thread_pool.hpp>
#include <unifex/sync_wait.hpp>
#include <unifex/task.hpp>
#include <unifex/then.hpp>
#include <unifex/via.hpp>
#include <unifex/when_all.hpp>
#include "cxx_async.h"
#include "cxx_async_libunifex.h"
#include "example.h"
#include "libunifex/src/main.rs.h"
#include "rust/cxx.h"

const size_t EXAMPLE_SPLIT_LIMIT = 32;
const size_t EXAMPLE_ARRAY_SIZE = 16384;

typedef decltype(unifex::static_thread_pool().get_scheduler()) UnifexThreadPoolScheduler;

// TODO(pcwalton): It'd be nice to be able to spawn the extra threads that `dot_product` creates on
// a Rust thread pool instead. There has to be some kind of thread pool for this example to actually
// test parallelism, though.
unifex::static_thread_pool g_thread_pool;
UnifexThreadPoolScheduler g_thread_pool_scheduler = g_thread_pool.get_scheduler();

static unifex::task<double> dot_product_inner(const double a[], const double b[], size_t count) {
    if (count > EXAMPLE_SPLIT_LIMIT) {
        size_t half_count = count / 2;
        auto taskA = [&]() -> unifex::task<double> {
            co_await unifex::schedule(g_thread_pool_scheduler);
            co_return co_await dot_product_inner(a, b, half_count);
        };
        auto taskB = [&]() -> unifex::task<double> {
            co_return co_await dot_product_inner(a + half_count, b + half_count,
                                                 count - half_count);
        };
        auto results = co_await unifex::when_all(taskA(), taskB());
        double a = std::get<0>(std::get<0>(std::get<0>(results)));
        double b = std::get<0>(std::get<0>(std::get<1>(results)));
        co_return a + b;
    }

    double sum = 0.0;
    for (size_t i = 0; i < count; i++)
        sum += a[i] * b[i];
    co_return sum;
}

static unifex::task<double> dot_product() {
    Xorshift rand;
    std::vector<double> array_a, array_b;
    for (size_t i = 0; i < EXAMPLE_ARRAY_SIZE; i++) {
        array_a.push_back((double)rand.next());
        array_b.push_back((double)rand.next());
    }

    co_return co_await dot_product_inner(&array_a[0], &array_b[0], array_a.size());
}

rust::Box<RustFutureF64> libunifex_dot_product() {
    co_return co_await dot_product();
}

void libunifex_call_rust_dot_product() {
    rust::Box<RustFutureF64> future = rust_dot_product();
    double result = *unifex::sync_wait(std::move(future));
    std::cout << result << std::endl;
}

void libunifex_schedule_rust_dot_product() {
    /*
    rust::Box<RustFutureF64> future = rust_dot_product();
    double result = unifex::sync_wait(unifex::schedule_on(g_thread_pool_scheduler,
    std::move(future))); std::cout << result << std::endl;
    */
}

rust::Box<RustFutureF64> libunifex_not_product() {
    if (true)
        throw std::runtime_error("kaboom");
    co_return 1.0;  // Just to make this function a coroutine.
}

void libunifex_call_rust_not_product() {
    try {
        rust::Box<RustFutureF64> oneshot_receiver = rust_not_product();
        unifex::sync_wait(std::move(oneshot_receiver));
        std::terminate();
    } catch (const std::exception& error) {
        std::cout << error.what() << std::endl;
    }
}

rust::Box<RustFutureString> libunifex_ping_pong(int i) {
    std::string string(co_await rust_libunifex_ping_pong(i));
    co_return std::move(string) + "pong ";
}
