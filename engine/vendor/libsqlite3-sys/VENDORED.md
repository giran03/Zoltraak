# Vendored libsqlite3-sys 0.30.1 with SQLite 3.54.1

This is `libsqlite3-sys` 0.30.1 from crates.io, unchanged except for the
bundled SQLite source in `sqlite3/`, which is replaced with SQLite 3.54.1.
Both `engine/Cargo.toml` and `app/src-tauri/Cargo.toml` point at it through
`[patch.crates-io]`.

## Why

`libsqlite3-sys` 0.30.1 bundles SQLite 3.46.0, which is affected by
[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459): SQLite
before 3.54.0 can return uninitialized page-cache memory when a database is
opened next to a `-wal` file with a different page size. The fix ships in
`libsqlite3-sys` 0.39, but no sqlx release accepts it (sqlx 0.8 requires
`^0.30.1`, sqlx 0.9 requires `<0.38`).

## What changed

| File | Source |
| --- | --- |
| `sqlite3/sqlite3.c`, `sqlite3/sqlite3.h`, `sqlite3/sqlite3ext.h` | SQLite 3.54.1 amalgamation, `https://www.sqlite.org/2026/sqlite-amalgamation-3540100.zip` (2 990 966 bytes, SHA3-256 `92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30`, as published on sqlite.org/download.html) |
| everything else | `libsqlite3-sys` 0.30.1 from crates.io, as published |

Left out because they are unused here: the SQLCipher copy (`sqlcipher/`, only
for the `bundled-sqlcipher` features) and the crate's upgrade scripts.

The pre-generated Rust bindings (`sqlite3/bindgen_bundled_version*.rs`) are
still the 3.46.0 ones. SQLite only adds to its C API between releases, so
every declared function still matches. The one visible difference is that the
`SQLITE_VERSION_NUMBER` constant reads 3046000, while `sqlite3_libversion()`
reports 3.54.1 at runtime.

## Removing it

Once sqlx accepts `libsqlite3-sys` 0.39 or later: upgrade sqlx, delete the
`[patch.crates-io]` entries in both manifests, delete `engine/vendor/`, and
regenerate both `Cargo.lock` files.

## Updating SQLite again

Download the newer amalgamation from sqlite.org/download.html, check its
SHA3-256 against the published hash, copy the three files into `sqlite3/`,
and update the table above.
