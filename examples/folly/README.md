# Folly Integration Example

This example demonstrates comprehensive async interoperability between Rust and C++ using the [Folly](https://github.com/facebook/folly) library. It showcases bidirectional async function calls, parallel computation, exception handling, stream processing, and complex communication patterns.

## Overview

The Folly example is a sophisticated demonstration of Rust-C++ async integration that covers:

- **Bidirectional Async Calls**: Both Rust calling C++ async functions and C++ calling Rust async functions
- **Parallel Computation**: Multithreaded dot product calculation using both Folly coroutines and futures
- **Exception Handling**: Proper propagation of exceptions across language boundaries
- **Stream Processing**: Async stream generators (FizzBuzz implementations)
- **Complex Communication**: Ping-pong pattern with recursive async calls
- **Resource Management**: Future dropping and coroutine lifecycle management

## Prerequisites

### System Requirements

- **C++20 compiler** (GCC 10+, Clang 11+, or MSVC 2019+)
- **Folly library** installed and available
- **Rust 2018 edition** or later

### Folly Installation

The build script uses the `find-folly` crate to automatically locate your Folly installation. Ensure Folly is properly installed on your system:

#### Ubuntu/Debian
```bash
sudo apt-get install libfolly-dev
```

#### macOS (with Homebrew)
```bash
brew install folly
```

#### Building from Source
Follow the [official Folly installation guide](https://github.com/facebook/folly#build) for your platform.

## Building and Running

### Build the Example
```bash
cd examples/folly
cargo build
```

### Run the Interactive Demo
```bash
cargo run
```

### Run Tests
```bash
cargo test
```

## Architecture

### Key Components

#### Rust Side (`src/main.rs`)
- **Future Types**: `RustFutureVoid`, `RustFutureF64`, `RustFutureString`, `RustFutureStringNamespaced`
- **Stream Types**: `RustStreamString`
- **Parallel Computation**: Recursive async dot product using `async-recursion`
- **Thread Pool**: Shared `ThreadPool` for concurrent execution

#### C++ Side (`src/folly_example.cpp`)
- **Folly Coroutines**: Using `folly::coro::Task` for async operations
- **Folly Futures**: Traditional future combinators with `folly::Future`
- **Thread Pool**: `folly::CPUThreadPoolExecutor` for parallel work
- **Exception Handling**: Custom exception types with proper propagation

#### Headers (`include/folly_example.h`)
- **Future Definitions**: `CXXASYNC_DEFINE_FUTURE` macros for type mapping
- **Stream Definitions**: `CXXASYNC_DEFINE_STREAM` macros
- **Custom Exception Handling**: Specialized `TryCatch` implementation

## Features Demonstrated

### 1. Bidirectional Async Calls

**Rust → C++**:
```rust
// Call C++ coroutine from Rust
let result = ffi::folly_dot_product_coro().await.unwrap();

// Call C++ future combinator from Rust  
let result = ffi::folly_dot_product_futures().await.unwrap();
```

**C++ → Rust**:
```cpp
// Call Rust async function from C++
auto future = rust_dot_product();
double result = co_await std::move(future);
```

### 2. Parallel Computation

Both languages implement the same parallel dot product algorithm:
- **Vector Size**: 16,384 elements  
- **Split Threshold**: 32 elements
- **Thread Pool**: 8 threads (C++), futures thread pool (Rust)
- **Algorithm**: Recursive divide-and-conquer with async/await

### 3. Exception Handling

**C++ Exceptions → Rust**:
```cpp
// C++ throws custom exception
throw MyException("kaboom");
```

```rust
// Rust catches as CxxAsyncException
match result {
    Err(err) => assert_eq!(err.what(), "kaboom"),
    Ok(_) => panic!("should have failed"),
}
```

**Rust Errors → C++**:
```rust
// Rust returns error
Err(CxxAsyncException::new("kapow".into()))
```

```cpp
// C++ catches exception
try {
    co_await rust_function();
} catch (const rust::async::Error& e) {
    std::cout << e.what() << std::endl; // "kapow"
}
```

### 4. Stream Processing

Async stream generators demonstrating:
- **Basic Streams**: FizzBuzz sequence generation
- **Nested Async**: Streams that internally await other futures
- **Exception Propagation**: Streams that throw exceptions mid-stream
- **Resource Cleanup**: Proper stream termination and cleanup

### 5. Complex Communication Patterns

**Ping-Pong Pattern**:
```rust
fn rust_folly_ping_pong(i: i32) -> RustFutureString {
    RustFutureString::infallible(async move {
        format!("{}ping ", 
            if i < 4 {
                ffi::folly_ping_pong(i + 1).await.unwrap()
            } else {
                "".to_owned()
            }
        )
    })
}
```

This creates a recursive call chain: Rust → C++ → Rust → C++ → ... until termination.

## Test Coverage

The example includes comprehensive tests covering:

### Core Functionality Tests
- `test_rust_calling_cpp_synchronously_coro()` - Rust → C++ coroutine calls
- `test_rust_calling_cpp_synchronously_futures()` - Rust → C++ future calls  
- `test_rust_calling_cpp_on_scheduler()` - Scheduled execution
- `test_cpp_calling_void_rust_synchronously()` - C++ → Rust void calls
- `test_cpp_calling_rust_synchronously()` - C++ → Rust value calls
- `test_cpp_calling_rust_on_scheduler()` - Scheduled Rust execution

### Error Handling Tests
- `test_cpp_async_functions_throwing_exceptions()` - C++ exception propagation
- `test_rust_async_functions_returning_errors()` - Rust error propagation

### Advanced Pattern Tests  
- `test_ping_pong()` - Recursive async communication
- `test_complete()` - Void future completion
- `test_dropping_futures()` - Resource cleanup
- `test_fizzbuzz()` - Basic stream processing
- `test_indirect_fizzbuzz()` - Nested async streams
- `test_streams_throwing_exceptions()` - Stream error handling
- `test_dropping_coroutines()` - Coroutine lifecycle management

## Implementation Notes

### Thread Safety
- All async operations are thread-safe
- Shared thread pools coordinate work between languages
- Proper synchronization for cross-language communication

### Memory Management
- Automatic resource cleanup for futures and streams
- RAII patterns ensure proper destruction
- No memory leaks in cross-language async operations

### Performance Considerations
- Minimal overhead for language boundary crossings
- Efficient parallel computation with proper work distribution
- Optimized for both throughput and latency

### Namespace Support
The example demonstrates C++ namespace mapping:
```cpp
CXXASYNC_DEFINE_FUTURE(::rust::String, foo, rust, bar, RustFutureStringNamespaced);
```

```rust
#[cxx_async::bridge(namespace = foo::rust::bar)]
unsafe impl Future for RustFutureStringNamespaced {
    type Output = StringNamespaced;
}
```

## Troubleshooting

### Common Build Issues

**Folly Not Found**:
```
error: Couldn't find the Folly library!
```
Solution: Ensure Folly is installed and `PKG_CONFIG_PATH` includes Folly's `.pc` file.

**C++20 Support**:
```
error: coroutines are a C++20 extension
```
Solution: Verify your compiler supports C++20 and the `--std=c++20` flag is properly set.

**Linker Errors**:
```
undefined reference to folly symbols
```
Solution: Ensure all Folly dependencies are properly linked. Check `find-folly` output.

### Runtime Issues

**Thread Pool Errors**: Ensure sufficient system resources for the configured thread pools.

**Segmentation Faults**: Usually indicate improper future/stream lifecycle management. Review the test patterns for proper usage.

## Contributing

When modifying this example:

1. **Maintain Test Coverage**: All new features must include comprehensive tests
2. **Document Changes**: Update this README for any new functionality
3. **Follow Patterns**: Use existing error handling and async patterns
4. **Verify Cross-Platform**: Test on multiple platforms if possible

## Related Examples

- [`examples/cppcoro`](../cppcoro/README.md) - Similar patterns using cppcoro instead of Folly
- [`examples/common`](../common/README.md) - Shared utilities and patterns

## License

This example is licensed under both MIT and Apache 2.0 licenses, following the same licensing as the parent project.