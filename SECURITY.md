# Security Policy

## 🛡️ Reporting a Vulnerability

**This is a hobby project maintained in my free time.**

If you discover a security vulnerability within `pea2pea`'s core logic, please report it privately. **DO NOT** open a public issue.

Please use [GitHub's "Report a vulnerability" feature](https://github.com/ljedrz/pea2pea/security/advisories/new).

I will review reports as my schedule allows. While I strive to maintain the library's stability, please do not expect immediate responses or enterprise-level SLAs.

## 🔍 Simplicity as Security

`pea2pea` is architected to be completely understandable.

* **Auditable by Design:** The codebase is minimal and linear. A single developer can audit the entire library in a single afternoon.
* **No Black Boxes:** There are no complex hidden state machines or "magic" behaviors. We encourage you to read the source code to understand exactly what you are deploying.
* **Debuggable:** Because the abstraction layer is thin, bugs are rarely buried deep in the stack. If something breaks, the cause is usually visible immediately in the local scope.

## 🎯 Scope: What is a "Security Issue"?

Because `pea2pea` is a micro-kernel, it handles low-level plumbing while you handle the logic. Please check if the issue falls within the library's scope before reporting.

### ✅ In Scope (Library Responsibility)
* **Resource Leaks:** Failure to clean up file descriptors, memory, or tasks after a disconnect.
* **Limit Bypassing:** Mechanics that allow an attacker to exceed configured `max_connections` or `max_connections_per_ip`.
* **Panic/Crash Vectors:** Malformed TCP packets that cause the internal node loop to panic (DoS).

### ❌ Out of Scope (User Responsibility)
* **Protocol Logic:** Bugs in your own implementations of the protocols.
* **Unencrypted Traffic:** `pea2pea` uses raw TCP by default. Lack of encryption is not a vulnerability (see the `tls` or `noise` examples for secure implementations).
* **Application-Level DoS:** If your message handler blocks the async runtime.

## ⚠️ Disclaimer & Vendoring

As stated in the [LICENSE](LICENSE), this software is provided **"as is"**, without warranty of any kind.

If you vendor this library, **you act as the maintainer for your copy.** You are responsible for monitoring this repository for security updates and applying them to your codebase.
