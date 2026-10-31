# Sayso's copy of gpui-pre-wgpu

This is `gpui-pre-wgpu` 0.3.9 from crates.io, with one change. `[patch.crates-io]` in the root `Cargo.toml` uses it in place of the crate. The license of this crate is Apache-2.0 (`LICENSE-APACHE`), also for the change.

## The change

`src/wgpu_context.rs` has the function `mesa_breaks_storage_reads` and a log line. `tests/sayso.rs` tests the function, and `Cargo.toml` names that test. `uses_webgl_instance_data` also returns true for llvmpipe with LLVM 22 or later. The renderer then keeps instance data in the texture of the WebGL2 path, in place of a storage buffer.

## Why

Mesa 26.0 (`lp_build_masked_gather` in `lp_bld_gather.c`) calls `llvm.masked.gather` with an alignment argument. LLVM 22 removed that argument, so the module is not valid, and llvmpipe ends with SIGSEGV when LLVM compiles it. A fragment shader that reads a storage buffer makes that call. The GPUI shaders do this, and the instance texture does not.

## When to remove it

Remove this folder, the `[patch.crates-io]` table, and `exclude` in the root `Cargo.toml` when the Mesa versions that distributions ship have the fix, or sooner if a gpui-kit update has its own answer to this problem. Before you update gpui-kit to a new `gpui-pre-wgpu`, make the change again in the new version, or remove it. Cargo does not use a patch for a different version, and it warns about the unused patch.

Check with `cargo test -p gpui-pre-wgpu --test sayso` and with a start on llvmpipe with LLVM 22 ([docs/vm-testing.md](../../docs/vm-testing.md)).
