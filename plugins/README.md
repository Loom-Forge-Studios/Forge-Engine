# plugins/

First-party plugins (Ch.32), one directory per plugin; each one proves I16 — it uses no capability a third-party plugin could not. Workspace membership is globbed
(`plugins/*`, D-6): create `plugins/<name>/Cargo.toml` and the crate is a member — do not edit
the root `Cargo.toml` members list.

A plugin directory is claimed by the `plugins/` entry of the master-plan repo tree; like
every crate it may only depend downward on the Ch.1.1 spine, inherits
the workspace lints (`[lints] workspace = true`), and registers an error-code prefix in
`docs/error-codes.md` before allocating its first code.

This file exists so the `plugins/*` glob always matches a real directory, even before the
first plugin lands.
