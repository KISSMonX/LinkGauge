# Third-Party Notices

## riperf3

The test engine is [riperf3](https://github.com/therealevanhenry/riperf3), a
wire-compatible, pure-Rust implementation of the iperf3 protocol, vendored
under `vendor/riperf3` with a small local patch (see the repo README).

- Copyright: Evan Henry
- License: MIT OR Apache-2.0
  - `vendor/riperf3/LICENSE-MIT.txt`
  - `vendor/riperf3/LICENSE-APACHE.txt`

Its dependencies (rsa, serde, tokio, socket2, etc.) are covered by their
respective licenses; see the Cargo.lock for the exact versions.

## windows-sys

Windows network-interface discovery uses the official Rust for Windows
`windows-sys` bindings to call the operating system's IP Helper API directly.
No PowerShell process or packet-capture runtime is required.

- Copyright: Microsoft Corporation
- License: MIT OR Apache-2.0

## russh

The SSH remote console uses [russh](https://github.com/warp-tech/russh), a
pure-Rust SSH client/server library, pulled from crates.io (no vendored copy).

- Copyright: russh contributors (originally Pierre-Étienne Meunier, thrussh)
- License: Apache-2.0

It is built with `default-features = false` plus the `ring`, `flate2` and `rsa`
features, so the cryptography backend is [ring](https://github.com/briansmith/ring)
(ISC-style license) rather than aws-lc-rs. See the Cargo.lock for exact versions
of these and their transitive dependencies.
