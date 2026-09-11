---
name: systems
title: Systems / Embedded / Native Intake
description: Before writing native, systems, or embedded code, retrieve current toolchain/standard versions, memory and concurrency rules, undefined-behavior and safety guidance, and the right build/flash/test commands — like a senior systems engineer briefing themselves.
triggers:
  keywords: [embedded, firmware, rtos, freertos, zephyr, microcontroller, "bare metal", "bare-metal", no_std, kernel, "device driver", "systems programming", cmake, "c++", cpp, stm32, esp32, arduino, "memory safety", "undefined behavior", simd, mmap, syscall]
  globs: ["CMakeLists.txt", "*.cpp", "*.cc", "*.hpp", "*.ino", "platformio.ini", "*.ld", "Kconfig", "prj.conf", "sdkconfig", "*.dts"]
---

# Systems / Embedded / Native Intake

Do this **before** writing or changing native/systems/embedded code. Don't rely on training
data — toolchains, language standards, and platform constraints change, and a memory or
concurrency bug here is often silent until it corrupts state or crashes in the field. Confirm
against the toolchain/standard docs and the project's own build config.

## Establish current state first

1. **Toolchain & standard in use** — read the build config (`CMakeLists.txt`, `Cargo.toml`,
   `platformio.ini`, `Makefile`): compiler + version, language standard (C11/C++20/Rust edition),
   target triple/MCU, and `no_std`/freestanding vs hosted. Match what's already there.
2. **Memory & ownership** — allocation strategy (heap vs static/stack, arenas, no-alloc on
   embedded), ownership/lifetime rules, and buffer-bounds discipline. Avoid undefined behavior:
   no use-after-free, no data races, no signed overflow, no aliasing violations.
3. **Concurrency & interrupts** — what runs in ISR vs task context, shared state and its locking
   (or lock-free/atomics), `volatile` for MMIO, and memory-ordering requirements.
4. **Platform constraints** — flash/RAM budget, alignment and endianness, real-time deadlines,
   and the ABI/calling convention if crossing language or FFI boundaries.
5. **Safety tooling** — what's available and expected: sanitizers (ASan/UBSan/TSan), static
   analysis (clang-tidy, cppcheck), `cargo clippy`/`miri`, and valgrind on hosted targets.

## Build & CLI tools

- Build: `cmake --build`, `make`, `cargo build --target ...`, `west build`, `idf.py`,
  `platformio run`. Use the project's presets/wrapper.
- Test/verify: `ctest`, `cargo test`/`clippy`/`miri`, unit tests on host, and on-target/HIL or
  an emulator (QEMU/Renode) when hardware isn't available.
- Flash/debug: the project's `openocd`/`gdb`/`probe-rs`/`idf.py flash` flow.

## What to record before coding

- The toolchain/standard/target and the exact build config you will use.
- The memory/ownership and concurrency model for the code you touch, and the UB you must avoid.
- The platform budget/constraints relevant to the change.
- The build/sanitizer/test commands (and on-target or emulator run) that prove correctness.

Don't broaden the change beyond the task — no incidental toolchain bumps or refactors across
unrelated modules (see the surgical-changes rule in core-engineering).
