# App unit tests

These tests are kept outside `src/app.rs` but are compiled as child modules of
`app` through `#[path = "..."]` declarations. This preserves access to private
application helpers without exposing them as production APIs.

Each directory groups tests by feature. Broader terminal areas use another
directory level for keyboard input, paste handling, protocol behavior,
selection, colors, and SFTP sorting.

`terminal_bench` is the exception to "test": it holds `#[ignore]`d benchmarks of
the terminal hot paths, each comparing the pre-optimization shape against the
current one inside a single binary. Run them (release gives numbers the parser
does not dominate) with:

```text
cargo +stable-x86_64-pc-windows-gnu test --release -- --ignored --nocapture bench_terminal
```
