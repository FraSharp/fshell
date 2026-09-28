# Native archive dependency notices

Release binaries statically include libarchive, liblzma, libzstd, liblz4, and libb2. Their licensing remains with their authors; fshell's GPL-3.0-or-later license is in the top-level `LICENSE` file.

- `libarchive-COPYING` is libarchive's distribution notice, including its summary of file-specific licenses (copied from libarchive 3.8.9).
- `xz-COPYING.0BSD` is the 0BSD license for liblzma code (copied from xz 5.8.4); consult the corresponding source package for file-specific exceptions.
- `zstd-LICENSE` is zstd's BSD license (copied from zstd 1.5.7).
- `lz4-LICENSE` is the BSD 2-Clause notice on the liblz4 header (from lz4 1.10.0).
- libb2 is distributed under CC0 1.0 Universal; no copyright notice is required.

When upgrading a dependency, verify the installed version's own license and update these notices as needed.
