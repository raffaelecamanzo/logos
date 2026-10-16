# security-ignore fixtures

`sorted.toml` is a `.logos/security-ignore.toml` in its canonical spelling:
entries sorted by fingerprint, one `[[ignore]]` table each, blank-line separated,
a dated expiry written as a native TOML date, one trailing newline. Parsing it and
serialising the result must reproduce it byte for byte (`cmp` clean); the second
entry's reason carries an escaped quote and an escaped backslash to pin the string
writer. The fingerprints are arbitrary 64-hex values, not real findings.
