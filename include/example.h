// cxx-async2/include/example.h

#ifndef CXX_ASYNC2_EXAMPLE_H
#define CXX_ASYNC2_EXAMPLE_H

#include <cstdint>

// Simple PRNG that can be easily duplicated on the Rust and C++ sides to ensure identical output.
class Xorshift {
    uint32_t m_state;
    Xorshift(Xorshift&) = delete;
    void operator=(Xorshift) = delete;

   public:
    Xorshift() : m_state(0x243f6a88) {}

    inline uint32_t next() {
        uint32_t x = m_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        m_state = x;
        return x;
    }
};

#endif
