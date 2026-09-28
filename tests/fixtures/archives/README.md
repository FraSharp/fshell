# Archive extraction fixtures

The seven small archives here contain `sub/hello.txt` with the bytes `hello archive\n`, except `hello.txt.xz`, which contains just the compressed stream. They are deliberately committed so the tests need no archiver command, network access, or second libarchive version at test runtime. `hello.zip`, `hello.7z`, `hello.tar`, `hello.tar.gz`, `hello.tar.xz`, and `hello.txt.xz` were written with libarchive2 0.2.1's `WriteArchive` API; `hello.tar.zst` is the same tar encoded by zstd 1.5.7. Malicious tar samples are constructed from ustar headers in `tests/extract_tests.rs` to make their paths and links explicit.

When refreshing these fixtures, keep the same single-file payload and retain the misleading-extension test. A generator must run in a separate binary from the integration test so its writer's libarchive symbols cannot interpose on the decoder under test.
