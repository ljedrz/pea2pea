# Contributing to pea2pea

Thank you for your interest in contributing to `pea2pea`!

To maintain the library's stability, performance, and simplicity, we adhere to a **strict scope policy**. Please read this document carefully before opening a Pull Request.

## 📐 Design Philosophy

`pea2pea` is designed as a **micro-kernel** for P2P networking. It provides the absolute minimum required to build a network: connection pooling, stream handling, and framing.

It is **not** a framework. It does not enforce specific high-level protocols (like Gossipsub, Kademlia, etc.). Those belong in the application layer (user-land), not in this library.

## ✅ What We Accept

We welcome contributions that harden the existing core without expanding the API surface area:

1.  **Bug Fixes:** Resolving panics, race conditions, or logic errors.
2.  **Performance Improvements:** Optimizations that reduce memory footprint or CPU usage.
3.  **Documentation:** Clarifications, typo fixes, or additional examples.
4.  **Tests** New cases for any untested features, improvements to structure.

## ❌ What We Do Not Accept

To keep the library lightweight and maintainable, we generally **reject**:

1.  **New Features:** If it can be implemented *on top* of `pea2pea`, it belongs in your application, not here.
2.  **Helper Functions:** Quality-of-life abstractions often lead to bloat. We prefer explicit, low-level control.
3.  **Specific Protocols:** Implementations of specific RPC or Gossip algorithms.
4.  **New Dependencies:** We strive to keep the dependency tree minimal.

## 📝 How to Contribute

1.  **Open an Issue First:** Unless it is a trivial typo fix, **please open an issue** to discuss your proposed change. This saves your time if the change falls outside our scope.
2.  **Run Tests:** Ensure all tests pass locally.
    ```bash
    cargo test
    ```
3.  **Format Code:** Ensure your code is formatted correctly, and that `clippy` is happy.
    ```bash
    cargo fmt --all -- --check
    cargo clippy --all-targets -- -D warnings
    ```

Thank you for helping us keep `pea2pea` robust and reliable!
