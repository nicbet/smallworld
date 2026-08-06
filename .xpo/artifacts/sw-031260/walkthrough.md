# Walkthrough: gate double_free_panics to debug builds

`BrickPool::free` guards against double-frees with `debug_assert_eq!` on the slot
generation — intentionally debug-only, since the check is on a hot allocator path.
The test `double_free_panics` asserted that guard via `#[should_panic]`, which
means in `--release` (where `debug_assertions` are off and the assert compiles
out) the test fails by *not* panicking. Nobody had run the engine suite in release
until sw-2230ee made release the measurement profile.

Fix: `#[cfg(debug_assertions)]` on the test (`brick_pool.rs`), with a comment
explaining why. Release suite: 30/30; debug suite: 31/31 including this test.
The product behavior is unchanged — the guard was always debug-only by design.