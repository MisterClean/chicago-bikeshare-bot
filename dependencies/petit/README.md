# Petit dependency lock

The deployed scheduler remains upstream Petit 0.2.0, source revision
`170dee43be847b29bc065f4265a4b3e16d4fb7fd`. This reviewed lockfile updates
its compatible dependencies, including bytes, time and crossbeam-epoch.
`scripts/prepare-petit` applies this lock before the locked native build.

The lock also resolves optional SQLx MySQL dependencies. RSA advisory
RUSTSEC-2023-0071 has no patched compatible release, but RSA is absent from
the Linux SQLite scheduler dependency graph. CI asserts that exclusion
before ignoring that one advisory for this lockfile. The unused API and
TUI features remain disabled. The serde_yaml dependency retains an
unmaintained unsafe-libyaml warning; no published vulnerability is reported
for it. Native scheduler success, failure, timeout and SQLite receipt
behavior are exercised by the existing integration smoke test.
