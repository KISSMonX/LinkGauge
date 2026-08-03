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
