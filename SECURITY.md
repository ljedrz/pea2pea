# Security Policy

## ☎️ Reporting a Vulnerability

**This is a hobby project maintained in my free time.**

If you discover a security vulnerability within `pea2pea`'s core logic, please report it privately. **DO NOT** open a public issue.

Please use [GitHub's "Report a vulnerability" feature](https://github.com/ljedrz/pea2pea/security/advisories/new).

I will review reports as my schedule allows. While I strive to maintain the library's stability, please do not expect immediate responses.

If your organization requires guaranteed response times, priority issue resolution, or enterprise SLAs, please contact me using the e-mail address in [Cargo.toml](Cargo.toml) to discuss commercial support.

## 🎯 Scope: What is a "Security Issue"?

Because `pea2pea` is a micro-kernel, it handles low-level plumbing while you handle the logic. Please check if the issue falls within the library's scope before reporting.

### ✅ In Scope (Library Responsibility)
* **Resource Leaks:** Failure to clean up file descriptors, memory, or tasks after a disconnect.
* **Limit Bypassing:** Mechanics that allow an attacker to exceed the configurable limits.
* **Crash Vectors:** Anything that can cause the internal node to panic.

### ❌ Out of Scope (User Responsibility)
* **Protocol Logic:** Bugs in your own implementations of the protocols.
* **Unencrypted Traffic:** `pea2pea` uses raw TCP by default. Lack of encryption is not a vulnerability (see the `tls` or `noise` examples for secure implementations).
* **Application-Level DoS:** If your message handler blocks the async runtime.

## ⚠️ Disclaimer & Vendoring

As stated in the licenses, this software is provided **"as is"**, without warranty of any kind.

If you vendor this library, **you act as the maintainer for your copy.** You are responsible for monitoring this repository for security updates and applying them to your codebase.
