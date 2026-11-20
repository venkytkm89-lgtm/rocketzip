# RocketZip

A 7-Zip-style archiver with its own maximum-compression format, **Ultra (.zfx)**.
Two self-contained programs. **Nothing needs to be installed first**: no Visual C++
Redistributable, no .NET, no Python, no DLLs. They run on any 64-bit Windows 10/11 PC:

- **`RocketZip.exe`**: desktop window (browse, create, extract, test, drag and drop)
- **`rocketzip-cli.exe`**: 7-Zip-style command line

RocketZip is a Rust rewrite of an earlier Python prototype. The two read each other's
archives, including Ultra and Max archives.

## Formats

| Format | Create | Extract |
|---|---|---|
| **ZFX (Ultra)**: smallest output | ✅ | ✅ |
| 7Z (LZMA2, solid, AES-256 + encrypted names) | ✅ | ✅ |
| ZIP (Deflate, Zstd, XZ, BZip2, Store; AES-256) | ✅ | ✅ (also LZMA, Deflate64, PPMd) |
| TAR, TAR.GZ, TAR.BZ2, TAR.XZ, TAR.ZST | ✅ | ✅ |
| GZ, BZ2, XZ, ZST (single file) | ✅ | ✅ |
| RAR (v4/v5, with passwords) | - | ✅ |
| DOCX, XLSX, PPTX, ODT, EPUB, JAR, APK (ZIP inside) | - | ✅ |

Formats are detected by content, so renamed archives still open.

## What makes Ultra smaller

- **Document recompression.** DOCX/XLSX/PPTX/ODT/EPUB/JAR are ZIPs of weakly compressed
  XML. Ultra unpacks them, compresses the raw content far better, and on extraction rebuilds
  the original file **byte for byte**. Both classic zlib and zlib-ng are built in, as is
  LibreOffice's block-parallel style.
- **Auto-tuned filters per file type.** Text, documents, programs (x86/ARM BCJ), audio and
  bitmaps (delta), binary and already-compressed data each get their own compressor
  settings, chosen by test-compressing a sample.
- **Deduplication.** Identical files are stored once.
- **Max engine** (`--max`): a context-mixing compressor of the PAQ family, chosen per file
  type whenever it beats LZMA2.
- **Big dictionaries** (`--dict 256m` up to `1536m`), and **SHA-256** checks on every file.

Measured on real files (size as % of original; 7z = LZMA2 Ultra, the method 7-Zip uses):

| Data | ZIP | 7z | Ultra | Ultra `--docs full` | Ultra `--max --docs full` |
|---|---|---|---|---|---|
| Office documents made by LibreOffice | 100% | 86% | 53%¹ | 53% | **44%** |
| Mixed 20 MB folder (docs, PDFs, audio, programs, code) | 85% | 78% | 77% | 63% | **61%** |
| 4.7 MB of source code | 26% | 19.2% | 19.3% | 19.3% | **16.1%** |

¹ On a normal PC. On the Linux test machine, LibreOffice used an unusual zlib-ng build that
can't be reproduced byte-exactly, so the default (exact) mode got 85% there.

Run `rocketzip-cli bench MyFolder` to compare against your installed 7-Zip and WinRAR on
your own files.

**`--docs` modes**
- `exact` (default): documents come back byte-identical. If a document's compression can't
  be reproduced exactly, it's stored as it is.
- `full`: also recompresses PDFs and documents whose compression can't be reproduced (e.g.
  many Microsoft Office files). Those come back with identical content, verified by each
  member's CRC, but different bytes. Use `rocketzip-cli t` on the target machine before
  deleting originals.
- `off`: no document recompression.

## Building on Windows

**One-time setup**
1. Install **Rust** from https://rustup.rs. When it offers to install the *Visual Studio
   C++ Build Tools*, accept. They provide the compiler, linker and CMake.
2. Install **Inno Setup 6.3+** (only needed for the installer): https://jrsoftware.org/isdl.php
   or `winget install JRSoftware.InnoSetup`

**Build:** double-click **`build_windows.bat`**. It builds and tests everything, then creates:
- `target\release\RocketZip.exe` and `target\release\rocketzip-cli.exe`
- `installer\Output\RocketZip-Setup-1.0.0.exe`

Or by hand: `cargo build --release`, then `ISCC installer\rocketzip.iss`.

**Without installing anything:** push this folder to a GitHub repository. The included
workflow (`.github/workflows/windows.yml`) builds the programs and the installer on
GitHub's Windows machines. Download them from the run's *Artifacts*. The workflow also:
- checks that the `.exe` files don't depend on the Visual C++ runtime
- starts the window on a machine with no graphics card, to prove it works anywhere
- installs, uses and uninstalls the installer silently, to prove it works
- **publishes a release** when you push a version tag (see *Publishing* below)

### No prerequisites
- **C runtime:** `.cargo/config.toml` builds the Microsoft C runtime into the `.exe` files
  (static CRT), so the Visual C++ Redistributable isn't needed.
- **Graphics:** the window draws with DirectX 12 (Vulkan or OpenGL as fallbacks). DirectX 12
  is part of Windows 10/11 and includes a software renderer, so it also works in virtual
  machines without a graphics card.
- **Everything else** (compression libraries, the Max engine, RAR support) is compiled into
  the programs.

### What the installer adds
- Start-menu shortcut, optional desktop icon, uninstaller
- Right-click **Add to RocketZip archive…** on files and folders; **Extract here** /
  **Extract to folder** on archives (on Windows 11: under *Show more options*)
- **Send to → RocketZip archive** for many selected files
- Double-click opens `.zfx`; RocketZip listed under *Open with* for other archive types
- Optional `rocketzip-cli` on PATH

## Building on Linux / macOS
```
cargo build --release      # needs Rust, a C/C++ compiler and CMake
cargo test --release
```

## Command line (7-Zip style)
```
rocketzip-cli a backup.zfx Documents/ -v                  # Ultra, with statistics
rocketzip-cli a backup.zfx Documents/ --max --docs full   # smallest possible
rocketzip-cli a backup.7z  Documents/ -p                  # 7z with password (prompts)
rocketzip-cli a backup.zip Documents/ -mm=zstd            # ZIP with Zstandard
rocketzip-cli a backup Documents/ --best                  # try all, keep smallest
rocketzip-cli x backup.zfx -oRestored                     # extract
rocketzip-cli l backup.zip                                # list
rocketzip-cli t backup.zfx                                # test (SHA-256 per file)
rocketzip-cli bench Documents/                            # vs 7-Zip / WinRAR
```

## Desktop window
`RocketZip.exe [archive]` opens an archive. You can also drop an archive onto the window to
open it, or drop files to compress them. Explorer's menus use
`--add FILES…`, `--extract-here ARCHIVES…` and `--extract-to ARCHIVES…`. The extract modes
show progress and close themselves when done.

## Security

Archives from strangers are hostile input. RocketZip defends against:

| Attack | Defence |
|---|---|
| "Zip slip": names like `../x`, `/etc/x`, `C:\x` | Blocked; also Windows-only tricks: `a/C:/x`, `.. ` (trailing dot/space), NTFS streams (`file:stream`), device names (`CON`, `NUL`, `COM1`…) |
| Writing through symbolic links / junctions (link chains in TAR, links planted in the destination) | Never writes through a link inside the destination; TAR links are created last and only if they stay inside; RAR links are not extracted |
| Hard links pointing outside | Copied only from regular files already inside the destination |
| Decompression bombs | Free disk space checked before extracting and every 64 MB during; stops with a warning before the disk fills |
| Crafted `.zfx` files (bad settings, giant sizes) | Validated; limits on segment, index and document sizes; errors instead of crashes |
| Hostile documents being archived (zip bombs inside DOCX) | Expansion capped at 1 GB per document, otherwise stored as is |
| Planted temp files | Archives are written to a random, exclusively created temp file, then renamed |
| setuid/setgid bits from archives | Removed; only normal permission bits are restored |

Every one of these has an attack test in `rocketzip/tests/security.rs`.

Good practice when using it:
- Prefer `-p` without a value (RocketZip asks for the password). A password typed as
  `-pSECRET` is visible to other programs on the PC and in your shell history.
- The `.exe` files and installer are not code-signed, so Windows SmartScreen warns about
  them. For public distribution, sign them with a code-signing certificate.
- The GitHub workflow runs `cargo audit` on every build to flag libraries with known
  vulnerabilities. Keep dependencies updated with `cargo update`.
- For extra assurance, pin the GitHub Actions in the workflow to commit hashes, and fuzz
  the readers (e.g. with `cargo fuzz`).

## Publishing

### 1. GitHub Releases (the download page)
1. Create a **public** GitHub repository named `rocketzip` and push this folder to it.
2. Optional: under *Settings → Secrets and variables → Actions → Variables*, add
   `PUBLISHER_NAME` (your name or company). It appears in *Installed apps*.
3. To release, set the version in `rocketzip/Cargo.toml` (for example `1.0.0`), commit,
   then push a matching tag:
   ```
   git tag v1.0.0
   git push origin v1.0.0
   ```
   The workflow builds, tests and installs everything, then creates the release with:
   - `RocketZip-Setup-1.0.0.exe`: the installer
   - `RocketZip-1.0.0-portable-x64.zip`: the programs, no install needed
   - `SHA256SUMS.txt`: checksums so users can verify their download

   If the tag doesn't match the version in `Cargo.toml`, the build stops before anything
   is published.

### 2. winget (`winget install RocketZip`)
First version, by hand, once:
1. Download `wingetcreate` from https://aka.ms/wingetcreate/latest
2. Run `wingetcreate new <link to RocketZip-Setup-1.0.0.exe on your release page>`.
   Choose the package ID `YourName.RocketZip`, fill in the questions (publisher, license
   *MIT*, short description), and let it open the pull request to `microsoft/winget-pkgs`.
3. Microsoft's checks run automatically. A reviewer merges it, usually within a few days.

Every later version, automatically:
1. Create a classic GitHub token with the `public_repo` scope
   (*GitHub → Settings → Developer settings → Personal access tokens*).
2. In this repository add the secret `WINGET_TOKEN` (the token) and the variable
   `WINGET_ID` (e.g. `YourName.RocketZip`).
3. From then on, each `v*` tag also sends the new version to winget.

### 3. Windows SmartScreen and code signing
The installer is not code-signed. On its first downloads, Windows SmartScreen will show
*"Windows protected your PC"*; users click **More info → Run anyway**. The warning fades
as the file gains reputation. To remove it, sign the `.exe` files with a code-signing
certificate (for example Azure Artifact Signing, or a certificate from a CA) and add a
signing step to the workflow before *Build installer*. winget accepts unsigned installers.

### 4. Before publishing, check
- [ ] Replace `The RocketZip authors` in `LICENSE` with your name, if you want
- [ ] Search the name in your country's trademark register (India: ipindia.gov.in,
      US: tmsearch.uspto.gov) and on the Microsoft Store
- [ ] `THIRD-PARTY-NOTICES.txt` is regenerated automatically by every build

## License
RocketZip is free software under the **MIT license** (see `LICENSE`). It includes
open-source components under their own licenses, listed in full in
`THIRD-PARTY-NOTICES.txt`; the installer copies that file to the install folder.
RAR extraction uses the UnRAR source code, whose license forbids using it to create a
RAR-compatible archiver; RocketZip only reads RAR files. See `PRIVACY.md` for the privacy
statement: RocketZip collects no data and never connects to the internet.

## Project layout
```
zfcm/                 Max engine (context mixing), byte-identical to the prototype's C engine
rocketzip/src/
  api.rs              create / extract / list / test / best
  formats.rs          format detection
  engines.rs          classic zlib + zlib-ng, for exact document rebuilding
  ultra/              Ultra format: precomp.rs (documents), mod.rs (writer/reader)
  zipfmt.rs tarfmt.rs sevenz.rs rarfmt.rs
  bench.rs            7-Zip / WinRAR comparison
  bin/RocketZip.rs     desktop window (egui)
  bin/rocketzip-cli.rs command line
rocketzip/tests/       end-to-end tests
installer/            Inno Setup script (Explorer integration)
tools/gen_notices.py  regenerates THIRD-PARTY-NOTICES.txt from the real dependencies
build_windows.bat     one-click Windows build
```

## Notes
- **ZIP + LZMA:** the ZIP writer offers XZ instead of LZMA, which has the same LZMA2
  strength and opens in 7-Zip. Reading LZMA ZIPs works.
- **Windows version:** 64-bit Windows 10 or 11 (the installer checks this).
- **RAR:** extraction uses the official UnRAR source. Creating RAR files isn't possible,
  because the RAR format's license doesn't allow it.
