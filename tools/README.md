# tools/

Engine binaries and services (Ch.26, Ch.30, Ch.37), one directory per crate. Workspace membership is globbed
(`tools/*`, D-6): create `tools/<name>/Cargo.toml` and the crate is a member — do not edit
the root `Cargo.toml` members list.

Every crate directory must be claimed in the master-plan repo tree with the chapter that
specifies it (`just plan-coverage`), may only depend downward on the Ch.1.1 spine, inherits
the workspace lints (`[lints] workspace = true`), and registers an error-code prefix in
`docs/error-codes.md` before allocating its first code.

This file exists so the `tools/*` glob always matches a real directory, even before the
first tool lands.
