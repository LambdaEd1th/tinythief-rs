# NOTICE

## Tiny Thief assets

`crates/tt-web/assets/` contains the Tiny Thief game content (SWF, XML, MP3) in the
same AES-256-CBC encrypted form as the iOS app bundle. **All rights to Tiny Thief
belong to Rovio Entertainment Ltd.** The assets are included solely for technical
research and personal preservation of an application that is no longer distributed.
They are **not** covered by this repository's MIT/Apache-2.0 license, and you may not
redistribute or use them commercially without permission from the rights holder.

If you are a rights holder and want this content removed, open an issue and it will
be deleted.

## Ruffle

`vendor/ruffle/` is a subset of the [Ruffle](https://github.com/ruffle-rs/ruffle)
project (tag `v0.7.1`), used to run the original AS3/SWF game logic. Ruffle is
dual-licensed MIT / Apache-2.0; see `vendor/ruffle/LICENSE.md` and the upstream
repository. Local modifications are documented in `docs/ruffle-patch.md`.

## Game engine / third-party libraries

`tt-host` links Rust crates from crates.io (wgpu, winit, cpal, aes, etc.); their
licenses are listed in `Cargo.lock` metadata and the respective crates.

## This repository

The host code (`crates/`, `scripts/`, `docs/`, workflows) is licensed under
MIT OR Apache-2.0 — see `LICENSE-MIT` and `LICENSE-APACHE`.
